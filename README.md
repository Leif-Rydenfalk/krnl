[![DocsBadge]][Docs]
[![Build Status](https://github.com/charles-r-earp/autograph/workflows/Continuous%20Integration/badge.svg?branch=main)](https://github.com/charles-r-earp/krnl/actions)

[Docs]: https://docs.rs/krnl
[DocsBadge]: https://docs.rs/krnl/badge.svg

# **krnl**
Safe, portable, high performance compute (GPGPU) kernels.

Developed for [**autograph**](https://github.com/charles-r-earp/autograph). 
- Similar functionality to CUDA and OpenCL.
- Supports GPU's and other Vulkan 1.2 capable devices.
- MacOS / iOS supported via [MoltenVK](https://github.com/KhronosGroup/MoltenVK).
- Kernels are written inline, entirely in Rust.
- Simple iterator patterns can be implemented without unsafe.
- Buffers on the host can be accessed natively as Vecs and slices.

# **krnlc**
Kernel compiler for **krnl**. 
- Built on [RustGPU](https://github.com/EmbarkStudios/rust-gpu)'s spirv-builder.
- Supports dependencies defined in Cargo.toml. 
- Uses [spirv-tools](https://github.com/EmbarkStudios/spirv-tools-rs) to validate and optimize. 
- Compiles to "krnl-cache.rs", so the crate will build on stable Rust.

# Example
```rust
use krnl::{
    anyhow::Result,
    buffer::{Buffer, Slice, SliceMut},
    device::Device,
    macros::module,
};

#[module]
mod kernels {
    #[cfg(not(target_arch = "spirv"))]
    use krnl::krnl_core;
    use krnl_core::macros::kernel;

    pub fn saxpy_impl(x: f32, alpha: f32, y: &mut f32) {
        *y += alpha * x;
    }

    // Item kernels for iterator patterns.
    #[kernel]
    pub fn saxpy(#[item] x: f32, alpha: f32, #[item] y: &mut f32) {
        saxpy_impl(x, alpha, y);
    }

    // General purpose kernels like CUDA / OpenCL.
    #[kernel]
    pub fn saxpy_global(#[global] x: Slice<f32>, alpha: f32, #[global] y: UnsafeSlice<f32>) {
        use krnl_core::buffer::UnsafeIndex;
        let mut index = kernel.global_id() as usize;
        while index < x.len().min(y.len()) {
            saxpy_impl(x[index], alpha, unsafe { y.unsafe_index_mut(index) });
            index += kernel.global_threads() as usize;
        }
    }
}

fn saxpy(x: Slice<f32>, alpha: f32, mut y: SliceMut<f32>) -> Result<()> {
    if let Some((x, y)) = x.as_host_slice().zip(y.as_host_slice_mut()) {
        for (x, y) in x.iter().copied().zip(y) {
            kernels::saxpy_impl(x, alpha, y);
        }
        return Ok(());
    } 
    kernels::saxpy::builder()?
        .with_threads(256)
        .build(y.device())?
        .dispatch(x, alpha, y) 
}

fn main() -> Result<()> {
    let x = vec![1f32];
    let alpha = 2f32;
    let y = vec![0f32];
    let device = Device::builder().build().ok().unwrap_or(Device::host());
    let x = Buffer::from(x).into_device(device.clone())?;
    let mut y = Buffer::from(y).into_device(device.clone())?;
    saxpy(x.as_slice(), alpha, y.as_slice_mut())?;
    let y = y.into_vec()?;
    println!("{y:?}");
    Ok(())
}
```

# Performance 
*NVIDIA GeForce GTX 1060 with Max-Q Design*

## alloc

|                  | `krnl`                    | `cuda`                             | `ocl`                             |
|:-----------------|:--------------------------|:-----------------------------------|:--------------------------------- |
| **`1,000,000`**  | `319.07 ns` (✅ **1.00x**) | `112.83 us` (❌ *353.62x slower*)   | `486.10 ns` (❌ *1.52x slower*)    |
| **`10,000,000`** | `318.22 ns` (✅ **1.00x**) | `1.11 ms` (❌ *3494.06x slower*)    | `493.02 ns` (❌ *1.55x slower*)    |
| **`64,000,000`** | `318.40 ns` (✅ **1.00x**) | `6.31 ms` (❌ *19803.98x slower*)   | `493.07 ns` (❌ *1.55x slower*)    |

## upload

|                  | `krnl`                    | `cuda`                           | `ocl`                             |
|:-----------------|:--------------------------|:---------------------------------|:--------------------------------- |
| **`1,000,000`**  | `339.76 us` (✅ **1.00x**) | `363.93 us` (✅ **1.07x slower**) | `789.44 us` (❌ *2.32x slower*)    |
| **`10,000,000`** | `4.90 ms` (✅ **1.00x**)   | `3.81 ms` (✅ **1.29x faster**)   | `8.84 ms` (❌ *1.80x slower*)      |
| **`64,000,000`** | `25.92 ms` (✅ **1.00x**)  | `24.58 ms` (✅ **1.05x faster**)  | `56.74 ms` (❌ *2.19x slower*)     |

## download

|                  | `krnl`                    | `cuda`                           | `ocl`                             |
|:-----------------|:--------------------------|:---------------------------------|:--------------------------------- |
| **`1,000,000`**  | `593.88 us` (✅ **1.00x**) | `461.01 us` (✅ **1.29x faster**) | `20.12 ms` (❌ *33.88x slower*)    |
| **`10,000,000`** | `5.66 ms` (✅ **1.00x**)   | `4.07 ms` (✅ **1.39x faster**)   | `20.13 ms` (❌ *3.55x slower*)     |
| **`64,000,000`** | `29.50 ms` (✅ **1.00x**)  | `25.71 ms` (✅ **1.15x faster**)  | `37.48 ms` (❌ *1.27x slower*)     |

## zero

|                  | `krnl`                    | `cuda`                           | `ocl`                             |
|:-----------------|:--------------------------|:---------------------------------|:--------------------------------- |
| **`1,000,000`**  | `38.49 us` (✅ **1.00x**)  | `25.31 us` (✅ **1.52x faster**)  | `35.16 us` (✅ **1.09x faster**)   |
| **`10,000,000`** | `254.52 us` (✅ **1.00x**) | `243.01 us` (✅ **1.05x faster**) | `252.41 us` (✅ **1.01x faster**)  |
| **`64,000,000`** | `1.54 ms` (✅ **1.00x**)   | `1.55 ms` (✅ **1.01x slower**)   | `1.56 ms` (✅ **1.02x slower**)    |

## saxpy

|                  | `krnl`                    | `cuda`                           | `ocl`                             |
|:-----------------|:--------------------------|:---------------------------------|:--------------------------------- |
| **`1,000,000`**  | `88.59 us` (✅ **1.00x**)  | `81.25 us` (✅ **1.09x faster**)  | `89.24 us` (✅ **1.01x slower**)   |
| **`10,000,000`** | `742.25 us` (✅ **1.00x**) | `770.35 us` (✅ **1.04x slower**) | `780.49 us` (✅ **1.05x slower**)  |
| **`64,000,000`** | `4.68 ms` (✅ **1.00x**)   | `4.91 ms` (✅ **1.05x slower**)   | `4.92 ms` (✅ **1.05x slower**)    |

# Recent Changes 
See [Releases.md](https://github.com/charles-r-earp/krnl/blob/main/Releases.md)

# License
Dual-licensed to be compatible with the Rust project.

Licensed under the Apache License, Version 2.0 http://www.apache.org/licenses/LICENSE-2.0 or the MIT license http://opensource.org/licenses/MIT, at your option. This file may not be copied, modified, or distributed except according to those terms.

# Contribution
Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions