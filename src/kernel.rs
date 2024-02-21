/*!

[Kernels](#kernels) are functions dispatched from the host that execute on the device. They
are declared within [modules](#modules), which create a shared scope between host and device.
[krnlc](#krnlc) collects all modules and compiles them. [krnl-core](krnl_core) is a shared core library
between both host and device.

```no_run
use krnl::{
    macros::module,
    anyhow::Result,
    device::Device,
    buffer::{Buffer, Slice, SliceMut},
};

#[module]
# #[krnl(no_build)]
mod kernels {
    #[cfg(not(target_arch = "spirv"))]
    use krnl::krnl_core;
    use krnl_core::macros::kernel;

    pub fn saxpy_impl(alpha: f32, x: f32, y: &mut f32) {
        *y += alpha * x;
    }

    // Item kernels for iterator patterns.
    #[kernel]
    pub fn saxpy(alpha: f32, #[item] x: f32, #[item] y: &mut f32) {
        saxpy_impl(alpha, x, y);
    }

    // General purpose kernels like CUDA / OpenCL.
    #[kernel]
    pub fn saxpy_global(alpha: f32, #[global] x: Slice<f32>, #[global] y: UnsafeSlice<f32>) {
        use krnl_core::buffer::UnsafeIndex;

        let global_id = kernel.global_id();
        if global_id < x.len().min(y.len()) {
            saxpy_impl(alpha, x[global_id], unsafe { y.unsafe_index_mut(global_id) });
        }
    }
}

fn saxpy(alpha: f32, x: Slice<f32>, mut y: SliceMut<f32>) -> Result<()> {
    if let Some((x, y)) = x.as_host_slice().zip(y.as_host_slice_mut()) {
        x.iter()
            .copied()
            .zip(y.iter_mut())
            .for_each(|(x, y)| kernels::saxpy_impl(alpha, x, y));
        return Ok(());
    }
    # if true {
    kernels::saxpy::builder()?
        .build(y.device())?
        .dispatch(alpha, x, y)
    # } else {
    // or
    kernels::saxpy_global::builder()?
        .build(y.device())?
        .with_global_threads(y.len() as u32)
        .dispatch(alpha, x, y)
    # }
}

fn main() -> Result<()> {
    let alpha = 2f32;
    let x = vec![1f32];
    let y = vec![0f32];
    let device = Device::builder().build().ok().unwrap_or(Device::host());
    let x = Buffer::from(x).into_device(device.clone())?;
    let mut y = Buffer::from(y).into_device(device.clone())?;
    saxpy(alpha, x.as_slice(), y.as_slice_mut())?;
    let y = y.into_vec()?;
    println!("{y:?}");
    Ok(())
}
```

# **krnlc**
[Kernels](#kernels) are compiled with **krnlc**.

Compile with `krnlc` or `krnlc -p my-crate`.

1. Runs the equivalent of [`cargo expand`](https://github.com/dtolnay/cargo-expand) to locate all modules.
2. Generates a device crate under \<target-dir\>/krnlc/crates/\<my-crate\>.
3. Compiles the device crate with [spirv-builder](https://docs.rs/crate/spirv-builder).
4. Processes the output, validates and optimizes with [spirv-tools](https://docs.rs/spirv-tools).
5. Writes out to "krnl-cache.rs", which is imported by [`module`](#modules) and [`kernel`](#kernels) macros.

The cache allows packages to build with stable Rust, without recompiling kernels downstream:
```text
__krnl_cache!("0.0.4", "
abZy8000000@}Rn2yGJu{w.WVIuQ#sT$h4DaGh)Tk%#sdtgN ..
..
");
```

If the version of **krnlc** is incompatible with the **krnl** version, [`module`](#modules)
will emit a compiler error.

## Toolchains
To locate [modules](#modules), **krnlc** will use the nightly toolchain. Install it with:
```text
rustup toolchain install nightly
```
To compile kernels with [spirv-builder](https://docs.rs/crate/spirv-builder), a specific nightly is required:
```text
rustup toolchain install nightly-2023-05-27
rustup component add --toolchain nightly-2023-05-27 rust-src rustc-dev llvm-tools-preview
```

## Installing
With spirv-tools from the [LunarG Vulkan SDK](https://www.lunarg.com/vulkan-sdk/) installed (will save significant compile time):
```text
cargo +nightly-2023-05-27 install krnlc --locked --no-default-features \
 --features use-installed-tools
```
Otherwise:
```text
cargo +nightly-2023-05-27 install krnlc --locked
```

## Metadata
**krnlc** can read metadata from Cargo.toml:
```toml
[package.metadata.krnlc]
# enable default features when locating modules
default-features = false
# features to enable when locating modules
features = ["zoom", "zap"]

[package.metadata.krnlc.dependencies]
# source is inherited from host target
foo = { default-features = false, features = ["foo"] }
# keys are inherited if not provided
bar = {}
# private dependency
baz = { path = "baz" }
```

[krnl-core](krnl_core) is automatically included as a dependency.

# Modules
The `module` macro declares a shared host and device scope that is visible to [krnlc](#krnlc).
The [spirv](#spirv) arch will be used by **krnlc** when compiling modules to for the device.
```no_run
use krnl::macros::module;

#[module]
# #[krnl(no_build)]
mod kernels {
    #[cfg(not(target_arch = "spirv"))]
    use krnl::krnl_core;
    use krnl_core::macros::kernel;

    #[kernel]
    pub fn foo() {}
}
```
Modules mut be within a module hierarchy, not within fn's or impl blocks.

## Attributes
Additonal options can be passed via attributes:
```no_run
# use krnl::macros::module;
# mod foo { pub(crate) use krnl; }
#[module]
// Does not compile the module with krnlc, used for krnl's docs.
#[krnl(no_build)]
 // Override path to krnl when it isn't a dependency.
#[krnl(crate=foo::krnl)]
mod kernels {
    /* .. */
}
```

## Imports
Functions and other items are visible to other modules, and can be imported:
```no_run
mod foo {
    # use krnl::macros::module;
    #[module]
    # #[krnl(no_build)]
    pub mod bar {
        pub struct Bar;
    }
}

# use krnl::macros::module;
#[module]
# #[krnl(no_build)]
mod baz {
    use super::foo::bar::Bar;
}

# fn main() {}
```

# Kernels
The `kernel` macro declares a function that executes on the device, dispatched from the host.
```no_run
# #[krnl::macros::module] #[krnl(no_build)] mod kernels {
# use krnl::macros::kernel;
#[kernel]
fn foo<
    // Specialization Constants
    const U: i32,
    const V: f32,
    const W: u32,
>(
    // Kernel
    /* kernel: Kernel or ItemKernel */
    // Global Buffers
    #[global] a: Slice<f32>,
    #[global] b: UnsafeSlice<i32>,
    // Items
    #[item] c: f64,
    #[item] d: &mut u64,
    // Push Constants
    e: u8,
    f: i32,
    // Group Buffers
    #[group] g: UnsafeSlice<f32, 100>,
    #[group] h: UnsafeSlice<i32, { (W * 10 + 1) as usize }>,
) {
    /* .. */
}
# }
```

# Items
Item kernels are a simple and safe abstraction for iterator patterns. Item kernels
have an implcit [ItemKernel](krnl_core::kernel::ItemKernel) argument.

Mapping a buffer with a fn:
```no_run
# #[krnl::macros::module] #[krnl(no_build)] mod kernels {
# use krnl::{macros::kernel, buffer::{Buffer, Slice}, anyhow::Result};
fn scale_to_f32_impl(x: u8) -> f32 {
    x as f32 / 255.
}

#[kernel]
fn scale_to_f32(#[item] x: u8, #[item] y: &mut f32) {
    *y = scale_to_f32_impl(x);
}

# fn foo(x: Slice<u8>) -> Result<Buffer<f32>> {
if let Some(x) = x.as_host_slice() {
    let y: Vec<f32> = x.iter().copied().map(scale_to_f32_impl).collect();
    Ok(Buffer::from(y))
} else {
    let mut y = Buffer::zeros(x.device(), x.len())?;
    scale_to_f32::builder()?
        .build(x.device())?
        .dispatch(x, y.as_slice_mut())?;
    Ok(y)
}
# }
# }
```

# Push Constants
Scalar arguments without an attribute. Unlike [SpecConstants](#Specialization), they are
provided to [`.dispatch(..)`](#dispatch), and do not require rebuilding the kernel.

At least 128 bytes of push constants can be used, depending on the device. Each [item](#items) or
[global](#global-buffers)  argument requires 8 bytes of push constants.

# Groups, Subgroups, and Threads
Kernels without [items](#items) have an implicit [Kernel](krnl_core::kernel::Kernel) argument that uniquely
identifies the group, subgroup, and thread.

Kernels are dispatched with groups of threads (CUDA thread blocks). Threads in a group are executed together,
typically on the same processor with a shared L1 cache. This is exposed via [Group Buffers](#group-buffers).

Thread groups are composed of subgroups of threads (CUDA warps), similar to SIMD vector registers on a CPU.
The number of threads per subgroup is a power of 2 between 1 and 128. Typical values are 32 for NVIDIA and 64
for AMD.
*/
//
// TODO: Intel Mesa driver uses variable subgroup size
// Fixed in https://github.com/charles-r-earp/krnl/tree/update-vulkano
// It can be accessed in a kernel via [`Kernel::subgroup_threads`](krnl_core::kernel::Kernel::subgroup_threads),
// or on the host via [`DeviceInfo::subgroup_threads()`](crate::device::DeviceInfo::subgroup_threads).
/*!

# Global Buffers
Visible to all threads. [Slice](krnl_core::buffer::Slice) binds to [Slice](crate::buffer::Slice), [UnsafeSlice](krnl_core::buffer::UnsafeSlice) binds
to [SliceMut](crate::buffer::SliceMut), provided to [`.dispatch(..)`](#dispatch).

For best performance, consecutive threads should access consecutive elements, allowing loads and stores to be coalesced
into fewer memory transactions.

# Group Buffers
Shared with all threads in the group, initialized with zeros. Can be used to minimize accesses
to [global buffers](#global-buffers).

The maximum amount of memory that can be used for group buffers depends on the device. Kernels
exceeding this will fail to [build](#kernel-builder).

Barriers should be used as necessary to synchronize access.
```no_run
# #[krnl::macros::module] #[krnl(no_build)] mod kernels {
# use krnl::macros::kernel;
#[kernel]
fn group_sum(
    #[global] x: Slice<f32>,
    #[group] x_group: UnsafeSlice<f32, 64>,
    #[global] y: UnsafeSlice<f32>,
) {
    use krnl_core::{
        buffer::UnsafeIndex,
        spirv_std::arch::workgroup_memory_barrier_with_group_sync as group_barrier
    };

    let global_id = kernel.global_id();
    let group_id = kernel.group_id();
    let thread_id = kernel.thread_id();
    unsafe {
        *x_group.unsafe_index_mut(thread_id) = x[global_id];
        // Barriers are used to synchronize access to group memory.
        // This call must be reached by all active threads in the group!
        group_barrier();
    }
    if thread_id == 0 {
        let mut acc = 0f32;
        for i in 0 .. 64 {
            unsafe {
                acc += *x_group.unsafe_index(i);
            }
        }
        unsafe {
            *y.unsafe_index_mut(group_id) = acc;
        }
    }
}
# }
```

# KernelBuilder
A [kernel declaration](#kernels) is expanded to a `mod` with a custom KernelBuilder and Kernel.

```
# #[cfg(target_arch = "spirv")]
pub mod saxpy {
    /// Builder for creating a [`Kernel`].
    ///
    /// See [`builder()`](builder).
    pub struct KernelBuilder { /* .. */ }

    /// Creates a builder.
    ///
    /// The builder is lazily created on first call.
    ///
    /// # Errors
    /// - The kernel wasn't compiled (with `#[krnl(no_build)]` applied to `#[module]`).
    pub fn builder() -> Result<KernelBuilder>;

    impl KernelBuilder {
        /// Threads per group.
        ///
        /// Defaults to [`DeviceInfo::default_threads()`](DeviceInfo::default_threads).
        pub fn with_threads(self, threads: u32) -> Self;
        /// Builds the kernel for `device`.
        ///
        /// The kernel is cached, so subsequent calls to `.build()` with identical
        /// builders (ie threads and spec constants) may avoid recompiling.
        ///
        /// # Errors
        /// - `device` doesn't have required features.
        /// - The kernel requires [specialization](kernel#specialization), but `.specialize(..)` was not called.
        /// - The kernel is not supported on `device`.
        /// - [`DeviceLost`].
        pub fn build(&self, device: Device) -> Result<Kernel>;
    }

    /// Kernel.
    pub struct Kernel<G = WithGroups<false>> { /* .. */ }

    impl<G> Kernel<G> {
        /// Threads per group.
        pub fn threads(&self) -> u32;
        /// Global threads to dispatch.
        ///
        /// Implicitly declares groups by rounding up to the next multiple of threads.
        pub fn with_global_threads(self, global_threads: u32) -> Kernel<WithGroups<true>>;
        /// Groups to dispatch.
        ///
        /// For item kernels, if not provided, is inferred based on item arguments.
        pub fn with_groups(self, groups: u32) -> Kernel<WithGroups<true>>;
    }

    impl Kernel<WithGroups<true>> {
        /// Dispatches the kernel.
        ///
        /// - Waits for immutable access to slice arguments.
        /// - Waits for mutable access to mutable slice arguments.
        /// - Blocks until the kernel is queued.
        ///
        /// # Errors
        /// - [`DeviceLost`].
        /// - The kernel could not be queued.
        pub fn dispatch(&self, alpha: f32, x: Slice<f32>, y: SliceMut<f32>) -> Result<()>;
    }
}
# fn main() {}
```
View the generated code and documentation with `cargo doc --open`.
Also use `--document-private-items` if the item is private.

The `builder()` method returns a KernelBuilder for creating a Kernel. This will fail if the
kernel wasn't compiled with [no_build](#attributes). The builder is cached so
that subsequent calls are trivial.

The number of threads per group can be set via `.with_threads(..)`. It will default to
[`DeviceInfo::default_threads()`](crate::device::DeviceInfo::default_threads) if not provided.

Building a kernel is an expensive operation, so it is cached within [Device](crate::device::Device). Subsequent
calls to `.build(..)` with identical builders (threads and [spec constants](#specialization)) may avoid recompiling.

# Features
Kernels implicitly declare [`Features`](device::Features) based on types and or operations used.
If the [device](device::Device) does not support these features, `.build(..)` will return an
error.

See [`DeviceInfo::features()`](device::DeviceInfo::features).

# Specialization
SpecConstants are declared like const generic parameters, but are not const when compiling
in Rust. They may be used to define the length of a [Group Buffer](#group-buffers). At runtime,
SpecConstants are provided to the [builder](#KernelBuilder) via `.specialize(..)`. During `.build(..)`,
they are converted to constants.
```no_run
# #[krnl::macros::module] #[krnl(no_build)] mod kernels {
# use krnl::macros::kernel;
#[repr(u32)]
enum Op {
    Add = 1,
    Sub = 2,
}

#[kernel]
fn binary<const OP: u32>(
    #[item] a: f32,
    #[item] b: f32,
    #[item] c: &mut f32,
) {
    if OP == Op::Add as u32 {
        *c = a + b
    } else if OP == Op::Sub as u32 {
        *c = a - b
    } else {
        panic!("Invalid op: {OP}");
    }
}

# fn build(device: krnl::device::Device) -> krnl::anyhow::Result<()> {
binary::builder()?
    .specialize(Op::Add as u32)
    .build(device)?;
# Ok(())
# }
# }
# fn main() {}
```

# Dispatch
Once [built](#KernelBuilder), the [groups](#groups-subgroups-and-threads) to dispatch may be set via `.with_groups(..)`,
or `.with_global_threads(..)` which rounds up to the next multiple of threads. [Item kernels](#items)
infer the global_threads based on the number of items.

The `.dispatch(..)` method blocks until the kernel is queued. One kernel can be queued
while another is executing.

When a kernel begins executing, the device will begin processing one or more groups
in parallel, untill all groups have finished.

Synchronization is automatically performed as necessary between kernels and when transfering buffers
to and from devices. [`Device::wait()`](crate::device::Device::wait) can be used to explicitly wait for prior operations to complete.

# SPIR-V
[Binary intermediate representation](https://www.khronos.org/spir) for graphics shaders that can be used with [Vulkan](https://www.vulkan.org).
[Kernels](#Kernels) are implemented as compute shaders targeting Vulkan 1.2.

[spirv-std](krnl_core::spirv_std) is a std library for the spirv arch, for use with [RustGPU](https://github.com/EmbarkStudios/rust-gpu).

## Asm
The [`asm!`](core::arch::asm) macro can be used with the spirv arch, see [inline-asm](https://github.com/EmbarkStudios/rust-gpu/blob/v0.9.0/docs/src/inline-asm.md).

# DebugPrintf
[debug_printf!](krnl_core::spirv_std::macros::debug_printfln) and [debug_printfln!](krnl_core::spirv_std::macros::debug_printfln)
will print formatted output to stderr.

```no_run
# #[krnl::macros::module] #[krnl(no_build)] mod kernels {
# use krnl::macros::kernel;
#[kernel]
fn foo(x: f32) {
    use krnl_core::spirv_std; // spirv_std must be in scope
    use spirv_std::macros::debug_printfln;

    unsafe {
        debug_printfln!("Hello World!");
    }
}
# }
```

Pass `--debug-printf` to [krnlc](#krnlc) to enable.  DebugPrintf will disable many optimizations and include
debug info, significantly increasing the size of both the cache and kernels at runtime.

The [DebugPrintf Validation Layer](https://github.com/KhronosGroup/Vulkan-ValidationLayers/blob/main/docs/debug_printf.md)
must be active when the [device](crate::device::Device) is created or DebugPrintf instructions will be removed.

```text
[Device(0@7f6f3c9724d0) crate::kernels::foo<threads=1>] Validation Information: [ UNASSIGNED-DEBUG-PRINTF ]
Object 0: handle = 0x7f6f3c9724d0, type = VK_OBJECT_TYPE_DEVICE; | MessageID = 0x92394c89 | Hello World!
```

# Panics

## Without [DebugPrintf](#DebugPrintf)
Panics in [kernels](#Kernels) will abort the thread. This will not stop other threads from continuing,
and the panic will not be caught from the host.

## With [DebugPrintf](#DebugPrintf)
Kernels will block on completion, and return an error on panic. When a kernel thread panics,
a message will be printed to stderr, including the device, the name, the panic message, and
a backtrace of calls leading to the panic.

```text
[Device(0@7f89289724d0) crate::kernels::foo<threads=2, N=4>] Validation Information: [ UNASSIGNED-DEBUG-PRINTF ] Object 0: handle = 0x7f89289b6070, type = VK_OBJECT_TYPE_QUEUE; | MessageID = 0x92394c89 | Command buffer (0x7f892896d7f0). Compute Dispatch Index 0. Pipeline (0x7f8928a95fb0). Shader Module (0x7f8928a9d500). Shader Instruction Index = 137.  Stage = Compute.  Global invocation ID (x, y, z) = (1, 0, 0 )
[Rust panicked at ~/.cargo/git/checkouts/krnl-699626729fecae20/db00d07/krnl-core/src/buffer.rs:169:20]
 index out of bounds: the len is 1 but the index is 1
      in <krnl_core::buffer::UnsafeSliceRepr<u32> as krnl_core::buffer::UnsafeIndex<usize>>::unsafe_index_mut
        called at ~/.cargo/git/checkouts/krnl-699626729fecae20/db00d07/krnl-core/src/buffer.rs:229:18
      by <krnl_core::buffer::BufferBase<krnl_core::buffer::UnsafeSliceRepr<u32>> as krnl_core::buffer::UnsafeIndex<usize>>::unsafe_index_mut
        called at src/kernels.rs:15:10
      by crate::kernels::foo::foo
        called at src/kernels.rs:11:1
      by crate::kernels::foo
        called at src/kernels.rs:12:8
      by crate::kernels::foo(__krnl_global_id = vec3(1, 0, 0), __krnl_groups = vec3(1, 1, 1), __krnl_group_id = vec3(0, 0, 0), __krnl_subgroups = 1, __krnl_subgroup_id = 0, __krnl_subgroup_threads = 32, __krnl_subgroup_thread_id = 1, __krnl_thread_id = vec3(1, 0, 0))
 Unable to find SPIR-V OpLine for source information.  Build shader with debug info to get source information.
thread 'foo' panicked at src/lib.rs:50:10:
called `Result::unwrap()` on an `Err` value: Kernel `crate::kernels::foo<threads=2, N=4>` panicked!
```

Note: The validation layer can be configured to redirect messages to stdout. This will prevent **krnl** from receiving a callback
and returning an error in case of a panic.
*/

use crate::{
    device::{Device, DeviceInner, Features},
    scalar::{ScalarElem, ScalarType},
};
use anyhow::{bail, Result};
#[cfg(feature = "device")]
use dry::macro_wrap;
#[cfg(feature = "device")]
use fxhash::FxHashMap;
#[cfg(feature = "device")]
use rspirv::{binary::Assemble, dr::Operand};
use std::{borrow::Cow, sync::Arc};
#[cfg(feature = "device")]
use std::{
    hash::Hash,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg_attr(not(feature = "device"), allow(dead_code))]
#[derive(Clone, Debug)]
pub(crate) struct KernelDesc {
    pub(crate) name: Cow<'static, str>,
    pub(crate) spirv: Vec<u32>,
    features: Features,
    pub(crate) threads: u32,
    spec_descs: &'static [SpecDesc],
    pub(crate) slice_descs: &'static [SliceDesc],
    push_descs: &'static [PushDesc],
}

#[cfg(feature = "device")]
impl KernelDesc {
    pub(crate) fn push_consts_range(&self) -> u32 {
        let mut size = 0;
        for push_desc in self.push_descs.iter() {
            while size % push_desc.scalar_type.size() != 0 {
                size += 1;
            }
            size += push_desc.scalar_type.size()
        }
        while size % 4 != 0 {
            size += 1;
        }
        size += self.slice_descs.len() * 2 * 4;
        size.try_into().unwrap()
    }
    fn specialize(
        &self,
        threads: u32,
        spec_consts: &[ScalarElem],
        debug_printf: bool,
    ) -> Result<Self> {
        use rspirv::spirv::{Decoration, Op};
        let mut module = rspirv::dr::load_words(&self.spirv).unwrap();
        let mut spec_ids = FxHashMap::<u32, u32>::default();
        spec_ids.reserve(spec_consts.len());
        let mut spec_string = format!("threads={threads}");
        use std::fmt::Write;
        for (desc, spec) in self.spec_descs.iter().zip(spec_consts) {
            if !spec_string.is_empty() {
                spec_string.push_str(", ");
            }
            let n = desc.name;
            macro_wrap!(match spec {
                macro_for!($T in [U8, I8, U16, I16, F16, BF16, U32, I32, F32, U64, I64, F64] {
                    ScalarElem::$T(x) => write!(&mut spec_string, "{n}={x}").unwrap(),
                })
                _ => unreachable!("{spec:?}"),
            });
        }
        let name = if !spec_string.is_empty() {
            format!("{}<{spec_string}>", self.name).into()
        } else {
            self.name.clone()
        };
        for inst in module.annotations.iter() {
            if inst.class.opcode == Op::Decorate {
                if let [Operand::IdRef(id), Operand::Decoration(Decoration::SpecId), Operand::LiteralInt32(spec_id)] =
                    inst.operands.as_slice()
                {
                    spec_ids.insert(*id, *spec_id);
                }
            }
        }
        for inst in module.types_global_values.iter_mut() {
            if inst.class.opcode == Op::SpecConstant {
                if let Some(result_id) = inst.result_id {
                    if let Some(spec_id) = spec_ids.get(&result_id).copied().map(|x| x as usize) {
                        let value = if let Some(value) = spec_consts.get(spec_id).copied() {
                            value
                        } else if spec_id == spec_consts.len() {
                            ScalarElem::U32(threads)
                        } else {
                            unreachable!("{inst:?}")
                        };
                        let bytes = value.as_bytes();
                        match inst.operands.as_mut_slice() {
                            [Operand::LiteralInt32(a)] => {
                                bytemuck::bytes_of_mut(a)[..bytes.len()].copy_from_slice(bytes);
                            }
                            [Operand::LiteralInt32(a), Operand::LiteralInt32(b)] => {
                                bytemuck::bytes_of_mut(a).copy_from_slice(&bytes[..8]);
                                bytemuck::bytes_of_mut(b).copy_from_slice(&bytes[9..]);
                            }
                            [Operand::LiteralFloat32(a)] => {
                                *a = f32::try_from(value).unwrap();
                            }
                            [Operand::LiteralFloat64(a)] => {
                                *a = f64::try_from(value).unwrap();
                            }
                            _ => unreachable!("{:?}", inst.operands),
                        }
                    }
                }
            }
        }
        if !debug_printf {
            strip_debug_printf(&mut module);
        }
        freeze_spec_constants(&mut module)?;
        reorder_push_constant_pointers(&mut module);
        let spirv = module.assemble();
        Ok(Self {
            name,
            spirv,
            spec_descs: &[],
            threads,
            ..self.clone()
        })
    }
}

#[cfg(feature = "device")]
fn strip_debug_printf(module: &mut rspirv::dr::Module) {
    use fxhash::FxHashSet;
    use rspirv::spirv::Op;

    module.extensions.retain(|inst| {
        inst.operands.first().unwrap().unwrap_literal_string() != "SPV_KHR_non_semantic_info"
    });
    let mut ext_insts = FxHashSet::default();
    module.ext_inst_imports.retain(|inst| {
        if inst
            .operands
            .first()
            .unwrap()
            .unwrap_literal_string()
            .starts_with("NonSemantic.DebugPrintf")
        {
            ext_insts.insert(inst.result_id.unwrap());
            false
        } else {
            true
        }
    });
    if ext_insts.is_empty() {
        return;
    }
    module.debug_string_source.clear();
    for func in module.functions.iter_mut() {
        for block in func.blocks.iter_mut() {
            block.instructions.retain(|inst| {
                if inst.class.opcode == Op::ExtInst {
                    let id = inst.operands.first().unwrap().unwrap_id_ref();
                    if ext_insts.contains(&id) {
                        return false;
                    }
                }
                !matches!(inst.class.opcode, Op::Line | Op::NoLine)
            })
        }
    }
}

// vulkano 0.34.1 false positive assert with spec constant ops where result type != constant type
// https://docs.rs/crate/vulkano/0.34.1/source/src/shader/spirv/specialization.rs
// evaluate_spec_constant_op constant_to_instruction
// no way to bypass because it attempts to specialize the module even with an empty map
// solution: replace OpSpecConstant with OpConstant and OpSpecConstantOp with the Op
// and remove SpecId decorations
// TODO: fixed in vulkano f2c68d7 https://github.com/vulkano-rs/vulkano/commit/f2c68d71c8e25a1b9ce35ab713a2960833c6115f
// remove when published.
#[cfg(feature = "device")]
fn freeze_spec_constants(module: &mut rspirv::dr::Module) -> Result<()> {
    use half::f16;
    use rspirv::{
        dr::Instruction,
        spirv::{Decoration, Op},
    };

    fn scalar_elem_from_operand(scalar_type: ScalarType, operand: &Operand) -> ScalarElem {
        match scalar_type {
            ScalarType::U8 => ScalarElem::U8(operand.unwrap_literal_int32().to_ne_bytes()[0]),
            ScalarType::I8 => ScalarElem::I8(i8::from_ne_bytes([operand
                .unwrap_literal_int32()
                .to_ne_bytes()[0]])),
            ScalarType::U16 => {
                let [a, b, _, _] = operand.unwrap_literal_int32().to_ne_bytes();
                ScalarElem::U16(u16::from_ne_bytes([a, b]))
            }
            ScalarType::I16 => {
                let [a, b, _, _] = operand.unwrap_literal_int32().to_ne_bytes();
                ScalarElem::U16(u16::from_ne_bytes([a, b]))
            }
            ScalarType::F16 => ScalarElem::F16(f16::from_f32(operand.unwrap_literal_float32())),
            ScalarType::U32 => ScalarElem::U32(operand.unwrap_literal_int32()),
            ScalarType::I32 => ScalarElem::I32(i32::from_ne_bytes(
                operand.unwrap_literal_int32().to_ne_bytes(),
            )),
            ScalarType::F32 => ScalarElem::F32(operand.unwrap_literal_float32()),
            ScalarType::U64 => ScalarElem::U64(operand.unwrap_literal_int64()),
            ScalarType::I64 => ScalarElem::I64(i64::from_ne_bytes(
                operand.unwrap_literal_int64().to_ne_bytes(),
            )),
            ScalarType::F64 => ScalarElem::F64(operand.unwrap_literal_float64()),
            _ => unreachable!(),
        }
    }

    fn scalar_elem_to_operand(scalar_elem: ScalarElem) -> Operand {
        match scalar_elem {
            ScalarElem::U8(x) => Operand::LiteralInt32(u32::from_ne_bytes([x, 0, 0, 0])),
            ScalarElem::I8(x) => {
                Operand::LiteralInt32(u32::from_ne_bytes([x.to_ne_bytes()[0], 0, 0, 0]))
            }
            ScalarElem::U16(x) => {
                let [a, b] = x.to_ne_bytes();
                Operand::LiteralInt32(u32::from_ne_bytes([a, b, 0, 0]))
            }
            ScalarElem::I16(x) => {
                let [a, b] = x.to_ne_bytes();
                Operand::LiteralInt32(u32::from_ne_bytes([a, b, 0, 0]))
            }
            ScalarElem::F16(x) => Operand::LiteralFloat32(x.to_f32()),
            ScalarElem::U32(x) => Operand::LiteralInt32(x),
            ScalarElem::I32(x) => Operand::LiteralInt32(u32::from_ne_bytes(x.to_ne_bytes())),
            ScalarElem::F32(x) => Operand::LiteralFloat32(x),
            ScalarElem::U64(x) => Operand::LiteralInt64(x),
            ScalarElem::I64(x) => Operand::LiteralInt64(u64::from_ne_bytes(x.to_ne_bytes())),
            ScalarElem::F64(x) => Operand::LiteralFloat64(x),
            _ => unreachable!(),
        }
    }

    use core::num::TryFromIntError;

    fn scalar_elem_try_from_int<T>(
        scalar_type: ScalarType,
        x: T,
    ) -> Result<ScalarElem, TryFromIntError>
    where
        T: std::fmt::Debug,
        u8: TryFrom<T>,
        TryFromIntError: From<<u8 as TryFrom<T>>::Error>,
        i8: TryFrom<T>,
        TryFromIntError: From<<i8 as TryFrom<T>>::Error>,
        u16: TryFrom<T>,
        TryFromIntError: From<<u16 as TryFrom<T>>::Error>,
        i16: TryFrom<T>,
        TryFromIntError: From<<i16 as TryFrom<T>>::Error>,
        u32: TryFrom<T>,
        TryFromIntError: From<<u32 as TryFrom<T>>::Error>,
        i32: TryFrom<T>,
        TryFromIntError: From<<i32 as TryFrom<T>>::Error>,
        u64: TryFrom<T>,
        TryFromIntError: From<<u64 as TryFrom<T>>::Error>,
        i64: TryFrom<T>,
        TryFromIntError: From<<i64 as TryFrom<T>>::Error>,
    {
        macro_wrap!(match scalar_type {
            macro_for!($T in [U8, I8, U16, I16, U32, I32, U64, I64] {
                ScalarType::$T => Ok(ScalarElem::$T(x.try_into()?)),
            })
            _ => unreachable!("{x:?} -> {scalar_type:?}"),
        })
    }

    macro_rules! binary_scalar_int_op {
        (|$a:ident, $b:ident| $e:expr) => {
            macro_wrap!(match ($a, $b) {
                macro_for!($T in [U8, I8, U16, I16, U32, I32, U64, I64] {
                    (ScalarElem::$T($a), ScalarElem::$T($b)) => $e,
                })
                _ => unreachable!("{:?}", ($a, $b)),
            })
        };
    }

    enum ScalarElemOrBool {
        ScalarElem(ScalarElem),
        Bool(bool),
    }

    impl From<ScalarElem> for ScalarElemOrBool {
        fn from(scalar_elem: ScalarElem) -> Self {
            Self::ScalarElem(scalar_elem)
        }
    }

    impl From<bool> for ScalarElemOrBool {
        fn from(input: bool) -> Self {
            Self::Bool(input)
        }
    }

    let mut scalars = FxHashMap::default();
    let mut values = FxHashMap::default();
    let mut bool_values = FxHashMap::default();
    for inst in module.types_global_values.iter_mut() {
        let op = inst.class.opcode;
        match op {
            Op::TypeInt => {
                let width = inst.operands[0].unwrap_literal_int32();
                let signed = inst.operands[1].unwrap_literal_int32() == 1;
                let scalar_type = match (width, signed) {
                    (8, false) => ScalarType::U8,
                    (8, true) => ScalarType::I8,
                    (16, false) => ScalarType::U16,
                    (16, true) => ScalarType::I16,
                    (32, false) => ScalarType::U32,
                    (32, true) => ScalarType::I32,
                    (64, false) => ScalarType::U64,
                    (64, true) => ScalarType::I64,
                    _ => unreachable!("{inst:?}"),
                };
                scalars.insert(inst.result_id.unwrap(), scalar_type);
            }
            Op::TypeFloat => {
                let width = inst.operands.first().unwrap().unwrap_literal_int32();
                let scalar_type = match width {
                    16 => ScalarType::F16,
                    32 => ScalarType::F32,
                    64 => ScalarType::F64,
                    _ => unreachable!(),
                };
                scalars.insert(inst.result_id.unwrap(), scalar_type);
            }
            Op::ConstantTrue | Op::SpecConstantTrue | Op::ConstantFalse | Op::SpecConstantFalse => {
                let value = matches!(op, Op::ConstantTrue | Op::SpecConstantTrue);
                bool_values.insert(inst.result_id.unwrap(), value);
                if let Op::SpecConstant = inst.class.opcode {
                    let op = if value {
                        Op::ConstantTrue
                    } else {
                        Op::ConstantFalse
                    };
                    let result_type = inst.result_type;
                    let result_id = inst.result_id;
                    let operands = Vec::new();
                    *inst = Instruction::new(op, result_type, result_id, operands);
                }
            }
            Op::Constant | Op::SpecConstant => {
                let operand = inst.operands.first().unwrap();
                let scalar_type = scalars[&inst.result_type.unwrap()];
                let value = scalar_elem_from_operand(scalar_type, operand);
                values.insert(inst.result_id.unwrap(), value);
                if let Op::SpecConstant = inst.class.opcode {
                    let result_type = inst.result_type;
                    let result_id = inst.result_id;
                    let operands = std::mem::take(&mut inst.operands);
                    *inst = Instruction::new(Op::Constant, result_type, result_id, operands);
                }
            }
            Op::SpecConstantComposite => {
                let result_type = inst.result_type;
                let result_id = inst.result_id;
                let operands = std::mem::take(&mut inst.operands);
                *inst = Instruction::new(Op::ConstantComposite, result_type, result_id, operands);
            }
            Op::SpecConstantOp => {
                let op = inst
                    .operands
                    .first()
                    .unwrap()
                    .unwrap_literal_spec_constant_op_integer();
                let operands = &inst.operands[1..];
                let result_type = inst.result_type;
                let result_id = inst.result_id;
                use ScalarElem as E;
                use ScalarType as T;
                let output = match op {
                    Op::UConvert | Op::SConvert => {
                        let scalar_type = scalars[&result_type.unwrap()];
                        let x = values[&operands.first().unwrap().unwrap_id_ref()];
                        macro_wrap!(match scalar_type {
                            macro_for!($T in [U8, I8, U16, I16, U32, I32, U64, I64] {
                                #[allow(clippy::useless_conversion, clippy::unnecessary_fallible_conversions)]
                                ScalarType::$T => ScalarElem::$T(match x {
                                    E::U8(x) => x.try_into()?,
                                    E::I8(x) => x.try_into()?,
                                    E::U16(x) => x.try_into()?,
                                    E::I16(x) => x.try_into()?,
                                    E::U32(x) => x.try_into()?,
                                    E::I32(x) => x.try_into()?,
                                    E::U64(x) => x.try_into()?,
                                    E::I64(x) => x.try_into()?,
                                    _ => unreachable!("{x:?}"),
                                }),
                            })
                             _ => unreachable!("{scalar_type:?}"),
                        })
                        .into()
                    }
                    Op::FConvert => {
                        let scalar_type = scalars[&result_type.unwrap()];
                        let x = values[&operands.first().unwrap().unwrap_id_ref()];
                        match (x, scalar_type) {
                            (E::F32(x), T::F64) => E::F64(x.into()),
                            (E::F64(x), T::F32) => E::F32(x as f32),
                            _ => unreachable!("{x:?} -> {scalar_type:?}"),
                        }
                        .into()
                    }
                    Op::SNegate => {
                        use std::ops::Neg;

                        let scalar_type = scalars[&result_type.unwrap()];
                        let x = values[&operands.first().unwrap().unwrap_id_ref()];
                        macro_rules! sneg {
                            ($U:ident, $I:ident, $i:ident) => {
                                match (x, scalar_type) {
                                    (E::$U(x), T::$U) => E::$U($i::try_from(x)?.neg().try_into()?),
                                    (E::$U(x), T::$I) => E::$I($i::try_from(x)?.neg()),
                                    (E::$I(x), T::$U) => E::$U(x.neg().try_into()?),
                                    (E::$I(x), T::$I) => E::$I(x.neg()),
                                    _ => unreachable!("{x:?} -> {scalar_type:?}"),
                                }
                            };
                        }

                        match (x.scalar_type().size(), scalar_type.size()) {
                            (1, 1) => sneg!(U8, I8, i8),
                            (2, 2) => sneg!(U16, I16, i16),
                            (4, 4) => sneg!(U32, I32, i32),
                            (8, 8) => sneg!(U64, I64, i64),
                            _ => unreachable!("{x:?} -> {scalar_type:?}"),
                        }
                        .into()
                    }

                    Op::Not => {
                        use std::ops::Not;

                        let scalar_type = scalars[&result_type.unwrap()];
                        let x = values[&operands.first().unwrap().unwrap_id_ref()];
                        macro_rules! not {
                            ($U:ident, $I:ident) => {
                                match (x, scalar_type) {
                                    (E::$U(x), T::$U) => E::$U(x.not().try_into()?),
                                    (E::$U(x), T::$I) => E::$I(x.not().try_into()?),
                                    (E::$I(x), T::$U) => E::$U(x.not().try_into()?),
                                    (E::$I(x), T::$I) => E::$I(x.not().try_into()?),
                                    _ => unreachable!("{x:?} -> {scalar_type:?}"),
                                }
                            };
                        }

                        match (x.scalar_type().size(), scalar_type.size()) {
                            (1, 1) => not!(U8, I8),
                            (2, 2) => not!(U16, I16),
                            (4, 4) => not!(U32, I32),
                            (8, 8) => not!(U64, I64),
                            _ => unreachable!("{x:?} -> {scalar_type:?}"),
                        }
                        .into()
                    }

                    Op::IAdd
                    | Op::ISub
                    | Op::IMul
                    | Op::UDiv
                    | Op::SDiv
                    | Op::UMod
                    | Op::SRem
                    | Op::SMod => {
                        let scalar_type = scalars[&result_type.unwrap()];
                        let a = values[&operands[0].unwrap_id_ref()];
                        let b = values[&operands[1].unwrap_id_ref()];
                        binary_scalar_int_op!(|a, b| scalar_elem_try_from_int(
                            scalar_type,
                            match op {
                                Op::IAdd => a + b,
                                Op::ISub => a - b,
                                Op::IMul => a * b,
                                Op::UDiv | Op::SDiv => a / b,
                                Op::UMod | Op::SMod => a % b,
                                Op::SRem => core::ops::Rem::rem(a, b),
                                _ => unreachable!(),
                            }
                        )?)
                        .into()
                    }

                    Op::Select => {
                        let condition = bool_values[&operands[0].unwrap_id_ref()];
                        let index = if condition { 1 } else { 2 };
                        if scalars.contains_key(&result_type.unwrap()) {
                            values[&operands[index].unwrap_id_ref()].into()
                        } else {
                            bool_values[&operands[index].unwrap_id_ref()].into()
                        }
                    }
                    Op::IEqual
                    | Op::INotEqual
                    | Op::ULessThan
                    | Op::SLessThan
                    | Op::UGreaterThan
                    | Op::SGreaterThan
                    | Op::ULessThanEqual
                    | Op::SLessThanEqual
                    | Op::UGreaterThanEqual
                    | Op::SGreaterThanEqual => {
                        let a = values[&operands[0].unwrap_id_ref()];
                        let b = values[&operands[1].unwrap_id_ref()];
                        binary_scalar_int_op!(|a, b| match op {
                            Op::IEqual => a == b,
                            Op::INotEqual => a != b,
                            Op::ULessThan | Op::SLessThan => a < b,
                            Op::UGreaterThan | Op::SGreaterThan => a > b,
                            Op::ULessThanEqual | Op::SLessThanEqual => a <= b,
                            Op::UGreaterThanEqual | Op::SGreaterThanEqual => a >= b,
                            _ => unreachable!(),
                        })
                        .into()
                    }

                    Op::ShiftRightLogical
                    | Op::ShiftRightArithmetic
                    | Op::ShiftLeftLogical
                    | Op::BitwiseOr
                    | Op::BitwiseXor
                    | Op::BitwiseAnd
                    | Op::VectorShuffle
                    | Op::CompositeExtract
                    | Op::CompositeInsert
                    | Op::LogicalOr
                    | Op::LogicalAnd
                    | Op::LogicalNot
                    | Op::LogicalEqual
                    | Op::LogicalNotEqual => bail!("SpecConstantOp {op:?} is unimplemented!"),
                    _ => unreachable!("{op:?}"),
                };
                match output {
                    ScalarElemOrBool::ScalarElem(output) => {
                        values.insert(result_id.unwrap(), output);
                        *inst = Instruction::new(
                            Op::Constant,
                            result_type,
                            result_id,
                            vec![scalar_elem_to_operand(output)],
                        );
                    }
                    ScalarElemOrBool::Bool(output) => {
                        bool_values.insert(result_id.unwrap(), output);
                        let op = if output {
                            Op::ConstantTrue
                        } else {
                            Op::ConstantFalse
                        };
                        *inst = Instruction::new(op, result_type, result_id, Vec::new());
                    }
                }
            }
            _ => (),
        }
    }
    module.annotations.retain(|inst| {
        if inst.class.opcode == Op::Decorate {
            if let [_id, Operand::Decoration(Decoration::SpecId), _spec_id] =
                inst.operands.as_slice()
            {
                return false;
            }
        }
        true
    });
    Ok(())
}

// vulkano 0.34.1 false positive assert with PushConstant TypePointer not being a struct
// https://docs.rs/vulkano/0.34.1/vulkano/shader/reflect/fn.entry_points.html
// solution: move non struct PushConstant TypePointer instructions to end of types_global_variables
// TODO: fixed in vulkano 84c6dbe https://github.com/vulkano-rs/vulkano/commit/84c6dbe18b26219cbbcc6ccac48f11beba86a27f
// remove when published.
#[cfg(feature = "device")]
fn reorder_push_constant_pointers(module: &mut rspirv::dr::Module) {
    use rspirv::{
        dr::Instruction,
        spirv::{Op, StorageClass},
    };

    let mut structs = Vec::new();
    let non_struct_pointers: Vec<_> = module.types_global_values.iter_mut().filter_map(|inst| {
        match inst.class.opcode {
            Op::TypeStruct => {
                structs.push(inst.result_id.unwrap());
            }
            Op::TypePointer => {
                if let [Operand::StorageClass(StorageClass::PushConstant), Operand::IdRef(pointee)] =
                    inst.operands.as_slice()
                {
                    if !structs.iter().any(|x| x == pointee) {
                        return Some(std::mem::replace(
                            inst,
                            Instruction::new(Op::Nop, None, None, Vec::new()),
                        ));
                    }
                }
            }
            _ => (),
        }
        None
    }).collect();
    module.types_global_values = module
        .types_global_values
        .drain(..)
        .filter(|inst| inst.class.opcode != Op::Nop)
        .chain(non_struct_pointers)
        .collect();
}

#[cfg(feature = "device")]
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) struct KernelKey {
    id: usize,
    spec_bytes: Vec<u8>,
}

#[doc(hidden)]
pub mod __private {
    #[cfg(feature = "device")]
    use num_traits::ToPrimitive;

    use super::*;
    #[cfg(feature = "device")]
    use crate::device::{DeviceBuffer, RawKernel};
    use crate::{
        buffer::{ScalarSlice, ScalarSliceMut, Slice, SliceMut},
        scalar::Scalar,
    };

    #[derive(Clone, Copy)]
    pub struct KernelDesc {
        name: &'static str,
        spirv: &'static [u8],
        features: Features,
        safe: bool,
        spec_descs: &'static [SpecDesc],
        slice_descs: &'static [SliceDesc],
        push_descs: &'static [PushDesc],
    }

    #[derive(Clone, Copy)]
    pub struct KernelDescArgs {
        pub name: &'static str,
        pub spirv: &'static [u8],
        pub features: Features,
        pub safe: bool,
        pub spec_descs: &'static [SpecDesc],
        pub slice_descs: &'static [SliceDesc],
        pub push_descs: &'static [PushDesc],
    }

    const fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        let mut i = 0;
        while i < a.len() {
            if a[i] != b[i] {
                return false;
            }
            i += 1;
        }
        true
    }

    pub const fn find_kernel(name: &str, kernels: &[KernelDesc]) -> Option<KernelDesc> {
        let mut i = 0;
        while i < kernels.len() {
            if bytes_eq(name.as_bytes(), kernels[i].name.as_bytes()) {
                return Some(kernels[i]);
            }
            i += 1;
        }
        None
    }

    pub const fn validate_kernel(
        kernel: Option<Option<KernelDesc>>,
        safety: Safety,
        spec_descs: &[SpecDesc],
        slice_descs: &[SliceDesc],
        push_descs: &[PushDesc],
    ) -> Option<KernelDesc> {
        if let Some(kernel) = kernel {
            let success = if let Some(kernel) = kernel.as_ref() {
                kernel.check_declaration(safety, spec_descs, slice_descs, push_descs)
            } else {
                false
            };
            if !success {
                panic!("recompile with krnlc");
            }
            kernel
        } else {
            None
        }
    }

    impl KernelDesc {
        pub const fn from_args(args: KernelDescArgs) -> Self {
            let KernelDescArgs {
                name,
                spirv,
                features,
                safe,
                spec_descs,
                slice_descs,
                push_descs,
            } = args;
            Self {
                name,
                spirv,
                features,
                safe,
                spec_descs,
                slice_descs,
                push_descs,
            }
        }
        const fn check_declaration(
            &self,
            safety: Safety,
            spec_descs: &[SpecDesc],
            slice_descs: &[SliceDesc],
            push_descs: &[PushDesc],
        ) -> bool {
            if self.safe != safety.is_safe() {
                return false;
            }
            {
                if self.spec_descs.len() != spec_descs.len() {
                    return false;
                }
                let mut index = spec_descs.len();
                while index < spec_descs.len() {
                    if !self.spec_descs[index].const_eq(&spec_descs[index]) {
                        return false;
                    }
                    index += 1;
                }
            }
            {
                if self.slice_descs.len() != slice_descs.len() {
                    return false;
                }
                let mut index = slice_descs.len();
                while index < slice_descs.len() {
                    if !self.slice_descs[index].const_eq(&slice_descs[index]) {
                        return false;
                    }
                    index += 1;
                }
            }
            {
                if self.push_descs.len() != push_descs.len() {
                    return false;
                }
                let mut index = push_descs.len();
                while index < push_descs.len() {
                    if !self.push_descs[index].const_eq(&push_descs[index]) {
                        return false;
                    }
                    index += 1;
                }
            }
            true
        }
    }

    #[derive(Clone, Copy)]
    pub enum Safety {
        Safe,
        Unsafe,
    }

    impl Safety {
        const fn is_safe(&self) -> bool {
            matches!(self, Self::Safe)
        }
    }

    const fn scalar_type_const_eq(a: ScalarType, b: ScalarType) -> bool {
        a as u32 == b as u32
    }

    #[derive(Clone, Copy, Debug)]
    pub struct SpecDesc {
        pub name: &'static str,
        pub scalar_type: ScalarType,
    }

    impl SpecDesc {
        const fn const_eq(&self, other: &Self) -> bool {
            bytes_eq(self.name.as_bytes(), other.name.as_bytes())
                && scalar_type_const_eq(self.scalar_type, other.scalar_type)
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct SliceDesc {
        pub name: &'static str,
        pub scalar_type: ScalarType,
        pub mutable: bool,
        pub item: bool,
    }

    impl SliceDesc {
        const fn const_eq(&self, other: &Self) -> bool {
            bytes_eq(self.name.as_bytes(), other.name.as_bytes())
                && scalar_type_const_eq(self.scalar_type, other.scalar_type)
                && self.mutable == other.mutable
                && self.item == other.item
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct PushDesc {
        pub name: &'static str,
        pub scalar_type: ScalarType,
    }

    impl PushDesc {
        const fn const_eq(&self, other: &Self) -> bool {
            bytes_eq(self.name.as_bytes(), other.name.as_bytes())
                && scalar_type_const_eq(self.scalar_type, other.scalar_type)
        }
    }

    fn decode_spirv(name: &str, input: &[u8]) -> Result<Vec<u32>, String> {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut output = Vec::new();
        GzDecoder::new(bytemuck::cast_slice(input))
            .read_to_end(&mut output)
            .map_err(|e| format!("Kernel `{name}` failed to decode! {e}"))?;
        let output = output
            .chunks_exact(4)
            .map(|x| u32::from_ne_bytes(x.try_into().unwrap()))
            .collect();
        Ok(output)
    }

    pub enum Specialized<const S: bool> {}

    #[cfg_attr(not(feature = "device"), allow(dead_code))]
    #[derive(Clone)]
    pub struct KernelBuilder {
        id: usize,
        desc: Arc<super::KernelDesc>,
        spec_consts: Vec<ScalarElem>,
        threads: Option<u32>,
    }

    impl KernelBuilder {
        pub fn from_desc(desc: KernelDesc) -> Result<Self, String> {
            let KernelDesc {
                name,
                spirv,
                features,
                safe: _,
                spec_descs,
                slice_descs,
                push_descs,
            } = desc;
            let spirv = decode_spirv(name, spirv)?;
            let desc = super::KernelDesc {
                name: name.into(),
                spirv,
                features,
                threads: 0,
                spec_descs,
                slice_descs,
                push_descs,
            };
            Ok(Self {
                id: name.as_ptr() as usize,
                desc: desc.into(),
                spec_consts: Vec::new(),
                threads: None,
            })
        }
        pub fn with_threads(self, threads: u32) -> Self {
            Self {
                threads: Some(threads),
                ..self
            }
        }
        pub fn specialize(self, spec_consts: &[ScalarElem]) -> Self {
            debug_assert_eq!(spec_consts.len(), self.desc.spec_descs.len());
            #[cfg(debug_assertions)]
            for (spec_const, spec_desc) in
                spec_consts.iter().copied().zip(self.desc.spec_descs.iter())
            {
                assert_eq!(spec_const.scalar_type(), spec_desc.scalar_type);
            }
            Self {
                spec_consts: spec_consts.to_vec(),
                ..self
            }
        }
        pub fn build(&self, device: Device) -> Result<Kernel> {
            match device.inner() {
                DeviceInner::Host => {
                    bail!("Kernel `{}` expected device, found host!", self.desc.name);
                }
                #[cfg(feature = "device")]
                DeviceInner::Device(device) => {
                    let desc = &self.desc;
                    let name = &desc.name;
                    let features = desc.features;
                    let info = device.info();
                    let device_features = info.features();
                    if !device_features.contains(&features) {
                        bail!("Kernel {name} requires {features:?}, {device:?} has {device_features:?}!");
                    }
                    let threads = self.threads.unwrap_or(info.default_threads());
                    let max_threads = info.max_threads();
                    if threads > max_threads {
                        bail!("Kernel {name} threads {threads} is greater than max_threads {max_threads}!");
                    }
                    let spec_bytes = self
                        .spec_consts
                        .iter()
                        .flat_map(|x| x.as_bytes())
                        .copied()
                        .chain(threads.to_ne_bytes())
                        .collect();
                    let key = KernelKey {
                        id: self.id,
                        spec_bytes,
                    };
                    let debug_printf = info.debug_printf();
                    let inner = RawKernel::cached(device.clone(), key, || {
                        desc.specialize(threads, &self.spec_consts, debug_printf)
                            .map(Arc::new)
                    })?;
                    Ok(Kernel {
                        inner,
                        threads,
                        groups: None,
                    })
                }
            }
        }
    }

    pub enum WithGroups<const G: bool> {}

    #[derive(Clone)]
    pub struct Kernel {
        #[cfg(feature = "device")]
        inner: RawKernel,
        threads: u32,
        #[cfg(feature = "device")]
        groups: Option<u32>,
    }

    impl Kernel {
        pub fn threads(&self) -> u32 {
            self.threads
        }
        pub fn with_global_threads(self, global_threads: u32) -> Self {
            #[cfg(feature = "device")]
            {
                let desc = &self.inner.desc();
                let threads = desc.threads;
                let groups = global_threads / threads + u32::from(global_threads % threads != 0);
                self.with_groups(groups)
            }
            #[cfg(not(feature = "device"))]
            {
                let _ = global_threads;
                unreachable!()
            }
        }
        pub fn with_groups(self, groups: u32) -> Self {
            #[cfg(feature = "device")]
            {
                Self {
                    groups: Some(groups),
                    ..self
                }
            }
            #[cfg(not(feature = "device"))]
            {
                let _ = groups;
                unreachable!()
            }
        }
        pub unsafe fn dispatch(
            &self,
            slices: &[KernelSliceArg],
            push_consts: &[ScalarElem],
        ) -> Result<()> {
            #[cfg(feature = "device")]
            {
                let desc = &self.inner.desc();
                let kernel_name = &desc.name;
                let mut buffers = Vec::with_capacity(desc.slice_descs.len());
                let mut items: Option<u32> = None;
                let device = self.inner.device();
                let mut push_bytes = Vec::with_capacity(desc.push_consts_range() as usize);
                debug_assert_eq!(push_consts.len(), desc.push_descs.len());
                for (push, push_desc) in push_consts.iter().zip(desc.push_descs.iter()) {
                    debug_assert_eq!(push.scalar_type(), push_desc.scalar_type);
                    debug_assert_eq!(push_bytes.len() % push.scalar_type().size(), 0);
                    push_bytes.extend_from_slice(push.as_bytes());
                }
                while push_bytes.len() % 4 != 0 {
                    push_bytes.push(0);
                }
                for (slice, slice_desc) in slices.iter().zip(desc.slice_descs.iter()) {
                    debug_assert_eq!(slice.scalar_type(), slice_desc.scalar_type);
                    debug_assert!(!slice_desc.mutable || slice.mutable());
                    let slice_name = &slice_desc.name;
                    if slice.len() == 0 {
                        bail!("Kernel `{kernel_name}`.`{slice_name}` is empty!");
                    }
                    let buffer = if let Some(buffer) = slice.device_buffer() {
                        buffer
                    } else {
                        bail!("Kernel `{kernel_name}`.`{slice_name}` expected device, found host!");
                    };
                    let buffer_device = buffer.device();
                    if device != buffer_device {
                        bail!(
                            "Kernel `{kernel_name}`.`{slice_name}`, expected `{device:?}`, found {buffer_device:?}!"
                        );
                    }
                    buffers.push(buffer.clone());
                    if slice_desc.item {
                        items.replace(if let Some(items) = items {
                            items.min(slice.len() as u32)
                        } else {
                            slice.len() as u32
                        });
                    }
                    let width = slice_desc.scalar_type.size();
                    let offset = buffer.offset() / width;
                    let len = buffer.len() / width;
                    push_bytes.extend_from_slice(&offset.to_u32().unwrap().to_ne_bytes());
                    push_bytes.extend_from_slice(&len.to_u32().unwrap().to_ne_bytes());
                }
                let info = self.inner.device().info().clone();
                let max_groups = info.max_groups();
                let groups = if let Some(groups) = self.groups {
                    if groups > max_groups {
                        bail!("Kernel `{kernel_name}` groups {groups} is greater than max_groups {max_groups}!");
                    }
                    groups
                } else if let Some(items) = items {
                    let threads = self.threads;
                    let groups = items / threads + u32::from(items % threads != 0);
                    groups.min(max_groups)
                } else {
                    #[cfg(debug_assertions)]
                    unreachable!("groups not provided!");
                };
                let debug_printf_panic = if info.debug_printf() {
                    Some(Arc::new(AtomicBool::default()))
                } else {
                    None
                };
                unsafe {
                    self.inner.dispatch(
                        groups,
                        &buffers,
                        push_bytes,
                        debug_printf_panic.clone(),
                    )?;
                }
                if let Some(debug_printf_panic) = debug_printf_panic {
                    device.wait()?;
                    while Arc::strong_count(&debug_printf_panic) > 1 {
                        std::thread::yield_now();
                    }
                    if debug_printf_panic.load(Ordering::SeqCst) {
                        bail!("Kernel `{kernel_name}` panicked!");
                    }
                }
                Ok(())
            }
            #[cfg(not(feature = "device"))]
            {
                let _ = (slices, push_consts);
                unreachable!()
            }
        }
        pub fn features(&self) -> Features {
            #[cfg(feature = "device")]
            {
                return self.inner.desc().features;
            }
            #[cfg(not(feature = "device"))]
            {
                unreachable!()
            }
        }
    }

    #[doc(hidden)]
    pub enum KernelSliceArg<'a> {
        Slice(ScalarSlice<'a>),
        SliceMut(ScalarSliceMut<'a>),
    }

    #[cfg(feature = "device")]
    impl KernelSliceArg<'_> {
        fn scalar_type(&self) -> ScalarType {
            match self {
                Self::Slice(x) => x.scalar_type(),
                Self::SliceMut(x) => x.scalar_type(),
            }
        }
        fn mutable(&self) -> bool {
            match self {
                Self::Slice(_) => false,
                Self::SliceMut(_) => true,
            }
        }
        fn device_buffer(&self) -> Option<&DeviceBuffer> {
            match self {
                Self::Slice(x) => x.device_buffer(),
                Self::SliceMut(x) => x.device_buffer_mut(),
            }
        }
        fn len(&self) -> usize {
            match self {
                Self::Slice(x) => x.len(),
                Self::SliceMut(x) => x.len(),
            }
        }
    }

    impl<'a, T: Scalar> From<Slice<'a, T>> for KernelSliceArg<'a> {
        fn from(slice: Slice<'a, T>) -> Self {
            Self::Slice(slice.into())
        }
    }

    impl<'a, T: Scalar> From<SliceMut<'a, T>> for KernelSliceArg<'a> {
        fn from(slice: SliceMut<'a, T>) -> Self {
            Self::SliceMut(slice.into())
        }
    }
}

pub(crate) use __private::{PushDesc, SliceDesc, SpecDesc};
