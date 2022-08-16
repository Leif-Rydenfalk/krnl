#![allow(unused)]
use crate::{device::DeviceOptions, result::Result, scalar::Scalar};
use anyhow::anyhow;
use crossbeam_channel::{bounded, Receiver, Sender};
use once_cell::sync::OnceCell;
use parking_lot::{Mutex, RwLock};
use spirv::Capability;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    iter::once,
    pin::Pin,
    ptr::NonNull,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use vulkano::{
    buffer::{
        cpu_access::ReadLock,
        cpu_pool::CpuBufferPoolChunk,
        device_local::DeviceLocalBuffer,
        sys::{UnsafeBuffer, UnsafeBufferCreateInfo},
        BufferAccess, BufferCreationError, BufferInner, BufferSlice, BufferUsage,
        CpuAccessibleBuffer, CpuBufferPool,
    },
    command_buffer::{
        pool::{
            CommandPool, UnsafeCommandPool, UnsafeCommandPoolAlloc, UnsafeCommandPoolCreateInfo,
        },
        submit::SubmitCommandBufferBuilder,
        sys::{
            CommandBufferBeginInfo,
            //    UnsafeCommandBufferBuilderPipelineBarrier,
            UnsafeCommandBuffer,
            UnsafeCommandBufferBuilder,
        },
        CommandBufferLevel, CommandBufferUsage, CopyBufferInfo,
    },
    descriptor_set::pool::{standard::StdDescriptorPoolAlloc, DescriptorPool, DescriptorPoolAlloc},
    device::{
        physical::{MemoryType, PhysicalDevice, PhysicalDeviceType, QueueFamily},
        Device, DeviceCreateInfo, DeviceExtensions, DeviceOwned, Features, Queue, QueueCreateInfo,
    },
    instance::{Instance, InstanceCreateInfo, InstanceCreationError, InstanceExtensions, Version},
    memory::{
        pool::StdMemoryPool, DeviceMemory, DeviceMemoryAllocationError, MappedDeviceMemory,
        MemoryAllocateInfo,
    },
    //pipeline::{layout::PipelineLayoutPcRange, ComputePipeline, PipelineBindPoint, PipelineLayout},
    shader::{
        spirv::ExecutionModel, DescriptorRequirements, EntryPointInfo, ShaderExecution,
        ShaderInterface, ShaderModule, ShaderStages,
    },
    sync::{
        AccessFlags, BufferMemoryBarrier, DependencyInfo, Fence, FenceCreateInfo, PipelineStages,
        Semaphore,
    },
    DeviceSize,
    OomError,
};

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod molten {
    use ash::vk::Instance;
    use std::os::raw::{c_char, c_void};
    use vulkano::instance::loader::Loader;

    pub(super) struct AshMoltenLoader;

    unsafe impl Loader for AshMoltenLoader {
        fn get_instance_proc_addr(&self, instance: Instance, name: *const c_char) -> *const c_void {
            let entry = ash_molten::load();
            let ptr = unsafe { entry.get_instance_proc_addr(std::mem::transmute(instance), name) };
            if let Some(ptr) = ptr {
                unsafe { std::mem::transmute(ptr) }
            } else {
                std::ptr::null()
            }
        }
    }
}
#[cfg(any(target_os = "ios", target_os = "macos"))]
use molten::AshMoltenLoader;

struct Backend {
    instance: Arc<Instance>,
    engines: Vec<Mutex<Weak<Engine>>>,
}

impl Backend {
    fn get_or_try_init() -> Result<&'static Self, InstanceCreationError> {
        static BACKEND: OnceCell<Backend> = OnceCell::new();
        BACKEND.get_or_try_init(|| {
            #[allow(unused_mut)]
            let mut instance = Instance::new(InstanceCreateInfo::default());
            #[cfg(any(target_os = "ios", target_os = "macos"))]
            {
                use vulkano::instance::loader::FunctionPointers;
                if instance.is_err() {
                    let info = InstanceCreateInfo {
                        function_pointers: Some(FunctionPointers::new(Box::new(AshMoltenLoader))),
                        enumerate_portability: true,
                        ..InstanceCreateInfo::default()
                    };
                    instance = Instance::new(info);
                }
            }
            let instance = instance?;
            let engines = PhysicalDevice::enumerate(&instance)
                .map(|_| Mutex::default())
                .collect();
            Ok(Self { instance, engines })
        })
    }
}

fn capabilites_to_features(capabilites: &[Capability]) -> Features {
    use Capability::*;
    let mut f = Features::none();
    for cap in capabilites {
        match cap {
            VulkanMemoryModel => {
                f.vulkan_memory_model = true;
            }
            StorageBuffer8BitAccess => {
                f.storage_buffer8_bit_access = true;
            }
            StorageBuffer16BitAccess => {
                f.storage_buffer16_bit_access = true;
            }
            Int8 => {
                f.shader_int8 = true;
            }
            Int16 => {
                f.shader_int16 = true;
            }
            Int64 => {
                f.shader_int64 = true;
            }
            _ => todo!(),
        }
    }
    f
}

fn features_to_capabilites(features: &Features) -> Vec<Capability> {
    use Capability::*;
    let f = features;
    let mut caps = Vec::new();
    if f.vulkan_memory_model {
        caps.push(VulkanMemoryModel);
    }
    if f.storage_buffer8_bit_access {
        caps.push(StorageBuffer8BitAccess);
    }
    if f.storage_buffer16_bit_access {
        caps.push(StorageBuffer16BitAccess);
    }
    if f.shader_int8 {
        caps.push(Int8);
    }
    if f.shader_int16 {
        caps.push(Int16);
    }
    if f.shader_int64 {
        caps.push(Int64);
    }
    caps
}

fn get_compute_family<'a>(
    physical_device: &'a PhysicalDevice,
) -> Result<QueueFamily<'a>, anyhow::Error> {
    physical_device
        .queue_families()
        .find(|x| !x.supports_graphics() && x.supports_compute())
        .or_else(|| {
            physical_device
                .queue_families()
                .find(|x| x.supports_compute())
        })
        .ok_or_else(|| {
            anyhow!(
                "Device {} doesn't support compute!",
                physical_device.index()
            )
        })
}

#[derive(Clone, derive_more::Deref)]
pub(crate) struct ArcEngine {
    #[deref]
    engine: Arc<Engine>,
}

impl ArcEngine {
    pub(super) fn new(index: usize, options: &DeviceOptions) -> Result<Self, anyhow::Error> {
        Ok(Self {
            engine: Engine::new(index, options)?,
        })
    }
}

impl PartialEq for ArcEngine {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.engine, &other.engine)
    }
}

impl Eq for ArcEngine {}

pub(crate) struct Engine {
    device: Arc<Device>,
    buffer_allocator: BufferAllocator,
    //storage_allocator: StorageAllocator,
    //shader_modules: DashMap<ModuleId, Arc<ShaderModule>, FxBuildHasher>,
    //compute_cache: DashMap<(ModuleId, EntryId), ComputeCache, FxBuildHasher>,
    op_sender: Sender<Op>,
    done: Arc<AtomicBool>,
    runner_result: Arc<RwLock<Result<(), Arc<anyhow::Error>>>>,
}

impl Engine {
    fn new(index: usize, options: &DeviceOptions) -> Result<Arc<Self>, anyhow::Error> {
        let backend = Backend::get_or_try_init()?;
        let physical_device =
            PhysicalDevice::from_index(&backend.instance, index).ok_or_else(|| {
                anyhow!(
                    "Cannot create device at index {}, only {} devices!",
                    index,
                    backend.engines.len()
                )
            })?;
        let engine_guard = backend.engines[index].lock();
        if let Some(engine) = Weak::upgrade(&engine_guard) {
            return Ok(engine);
        }
        let compute_family = get_compute_family(&physical_device)?;
        let device_extensions = DeviceExtensions::none();
        let optimal_device_features = capabilites_to_features(&options.optimal_capabilities);
        let device_features = physical_device
            .supported_features()
            .intersection(&optimal_device_features);
        let mut queue_create_info = QueueCreateInfo::family(compute_family);
        queue_create_info.queues = vec![1f32];
        let device_create_info = DeviceCreateInfo {
            enabled_extensions: device_extensions,
            enabled_features: device_features,
            queue_create_infos: vec![queue_create_info],
            ..DeviceCreateInfo::default()
        };
        let (device, mut queues) = Device::new(physical_device, device_create_info)?;
        let queue = queues.next().unwrap();
        let buffer_allocator = BufferAllocator::new(device.clone())?;
        //let shader_modules = DashMap::<_, _, FxBuildHasher>::default();
        //let compute_cache = DashMap::<_, _, FxBuildHasher>::default();
        let (op_sender, op_receiver) = bounded(1_000);
        let done = Arc::new(AtomicBool::new(false));
        let runner_result = Arc::new(RwLock::new(Ok(())));
        let mut runner = Runner::new(queue, op_receiver, done.clone(), runner_result.clone())?;
        std::thread::Builder::new()
            .name(format!("device{}", index))
            .spawn(move || runner.run())?;
        Ok(Arc::new(Self {
            device,
            buffer_allocator,
            //shader_modules,
            //compute_cache,
            op_sender,
            done,
            runner_result,
        }))
    }
    pub(crate) fn index(&self) -> usize {
        self.device.physical_device().index()
    }
    // # Safety
    // Uninitialized.
    #[forbid(unsafe_op_in_unsafe_fn)]
    pub(crate) unsafe fn alloc(&self, len: usize) -> Result<Option<Arc<DeviceBuffer>>> {
        if len == 0 {
            Ok(None)
        } else if len > u32::MAX as usize {
            anyhow::bail!(
                "Device buffer size {}B is too large, max is {}B!",
                len,
                u32::MAX
            );
        } else {
            let buffer = self.buffer_allocator.alloc_device(len as u32)?;
            Ok(Some(buffer))
        }
    }
    pub(crate) fn upload(&self, bytes: &[u8]) -> Result<Option<Arc<DeviceBuffer>>> {
        let len = bytes.len();
        if len == 0 {
            Ok(None)
        } else if len > u32::MAX as usize {
            anyhow::bail!(
                "Device buffer size {}B is too large, max is {}B!",
                len,
                u32::MAX
            );
        } else {
            let mut src = self.buffer_allocator.alloc_host(len as u32)?;
            Arc::get_mut(&mut src).unwrap().write_slice(bytes)?;
            let buffer = self.buffer_allocator.alloc_device(len as u32)?;
            let upload = Upload {
                src,
                dst: buffer.inner.clone(),
            };
            self.op_sender.send(Op::Upload(upload))?;
            Ok(Some(buffer))
        }
    }
    pub(crate) fn download(&self, buffer: Arc<DeviceBuffer>) -> Result<HostBufferFuture> {
        let src = buffer.inner.clone();
        let dst = self.buffer_allocator.alloc_host(buffer.len() as u32)?;
        let download = Download {
            src,
            dst: dst.clone(),
        };
        self.op_sender.send(Op::Download(download))?;
        Ok(HostBufferFuture {
            host_buffer: Some(dst),
            runner_result: self.runner_result.clone(),
        })
    }
}

/*
#[derive(Debug)]
struct RawBuffer {
    chunk: Arc<Chunk>,
    buffer: Arc<UnsafeBuffer>,
    usage: BufferUsage,
    buffer_start: u32,
    offset: u8,
    pad: u8,
}

impl RawBuffer {
    fn new(device: Arc<Device>, alloc: &ChunkAlloc) -> Result<Arc<Self>> {
        let memory = &alloc.chunk.memory;
        let mut usage = BufferUsage::transfer_src() | BufferUsage::transfer_dst();
        if memory.kind() == MemoryKind::Device {
            usage = usage | BufferUsage::storage_buffer();
        }
        let block = alloc.block;
        let len = block.len();
        let alignment = device.physical_device().properties().min_storage_buffer_offset_alignment as u32;
        let buffer_start = (block.start / alignment) * alignment;
        let offset = block.start % alignment;
        let pad = if offset > 0 {
            alignment - offset
        } else {
            0
        };
        let buffer_len = offset + len + pad;
        let buffer = UnsafeBuffer::new(
            device,
            UnsafeBufferCreateInfo {
                size: buffer_len as DeviceSize,
                usage,
                ..Default::default()
            }
        )?;
        Ok(Arc::new(Self {
            chunk: alloc.chunk.clone(),
            buffer,
            usage,
            buffer_start,
            offset: offset as u8,
            pad: pad as u8,
        }))
    }
}*/

#[derive(Debug)]
pub(crate) struct HostBuffer {
    alloc: Arc<ChunkAlloc<HostMemory>>,
    len: u32,
}

impl HostBuffer {
    fn new(alloc: Arc<ChunkAlloc<HostMemory>>, len: u32) -> Result<Arc<Self>> {
        Ok(Arc::new(Self { alloc, len }))
    }
    pub(crate) fn read(&self) -> Result<&[u8]> {
        let start = self.alloc.block.start as DeviceSize;
        let end = start + self.len as DeviceSize;
        Ok(unsafe { self.alloc.memory().memory.read(start..end)? })
    }
    fn write_slice(&mut self, slice: &[u8]) -> Result<()> {
        let start = self.alloc.block.start as DeviceSize;
        let end = start + self.len as DeviceSize;
        let data = unsafe { self.alloc.memory().memory.write(start..end)? };
        data.copy_from_slice(slice);
        Ok(())
    }
    fn chunk_id(&self) -> usize {
        Arc::as_ptr(&self.alloc.chunk) as usize
    }
    fn start(&self) -> DeviceSize {
        self.alloc.block.start as DeviceSize
    }
}

unsafe impl DeviceOwned for HostBuffer {
    fn device(&self) -> &Arc<Device> {
        self.alloc.memory().buffer.device()
    }
}

unsafe impl BufferAccess for HostBuffer {
    fn inner(&self) -> BufferInner {
        BufferInner {
            buffer: &self.alloc.memory().buffer,
            offset: self.alloc.block.start as DeviceSize,
        }
    }
    fn size(&self) -> DeviceSize {
        self.alloc.block.len() as DeviceSize
    }
    fn usage(&self) -> &BufferUsage {
        &self.alloc.memory().usage
    }
}

#[derive(Debug)]
pub(crate) struct HostBufferFuture {
    host_buffer: Option<Arc<HostBuffer>>,
    runner_result: Arc<RwLock<Result<(), Arc<anyhow::Error>>>>,
}

impl Future for HostBufferFuture {
    type Output = Result<HostBuffer>;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let host_buffer = self.host_buffer.take().unwrap();
        match Arc::try_unwrap(host_buffer) {
            Ok(host_buffer) => {
                let result = self.runner_result.read().clone();
                if let Err(e) = result {
                    Poll::Ready(Err(anyhow::Error::msg(e)))
                } else {
                    Poll::Ready(Ok(host_buffer))
                }
            }
            Err(host_buffer) => {
                self.host_buffer.replace(host_buffer);
                Poll::Pending
            }
        }
    }
}

#[derive(Debug)]
struct DeviceBufferInner {
    chunk: Arc<Chunk<DeviceMemory>>,
    buffer: Arc<UnsafeBuffer>,
    usage: BufferUsage,
    buffer_start: u32,
    len: u32,
    offset: u8,
    pad: u8,
}

impl DeviceBufferInner {
    fn chunk_id(&self) -> usize {
        Arc::as_ptr(&self.chunk) as usize
    }
    fn start(&self) -> DeviceSize {
        self.buffer_start as DeviceSize
    }
}

unsafe impl DeviceOwned for DeviceBufferInner {
    fn device(&self) -> &Arc<Device> {
        self.buffer.device()
    }
}

unsafe impl BufferAccess for DeviceBufferInner {
    fn inner(&self) -> BufferInner {
        BufferInner {
            buffer: &self.buffer,
            offset: 0,
        }
    }
    fn size(&self) -> DeviceSize {
        self.buffer.size()
    }
    fn usage(&self) -> &BufferUsage {
        &self.usage
    }
}

#[derive(Debug)]
pub(crate) struct DeviceBuffer {
    alloc: Arc<ChunkAlloc<DeviceMemory>>,
    inner: Arc<DeviceBufferInner>,
}

impl DeviceBuffer {
    fn new(
        device: Arc<Device>,
        alloc: Arc<ChunkAlloc<DeviceMemory>>,
        len: u32,
    ) -> Result<Arc<Self>> {
        let usage = BufferUsage::transfer_src()
            | BufferUsage::transfer_dst()
            | BufferUsage::storage_buffer();
        let align = device
            .physical_device()
            .properties()
            .min_storage_buffer_offset_alignment as u32;
        let pad = len % align;
        let buffer_len = len + pad;
        let buffer = UnsafeBuffer::new(
            device,
            UnsafeBufferCreateInfo {
                size: buffer_len as DeviceSize,
                usage,
                ..Default::default()
            },
        )?;
        unsafe { buffer.bind_memory(alloc.memory(), 0)? };
        let inner = Arc::new(DeviceBufferInner {
            chunk: alloc.chunk.clone(),
            buffer,
            usage,
            buffer_start: 0,
            len,
            offset: 0,
            pad: pad as u8,
        });
        Ok(Arc::new(Self { alloc, inner }))
    }
    pub(crate) fn len(&self) -> usize {
        self.inner.len as usize
    }
}

#[derive(Debug, Clone, Copy)]
struct Block {
    start: u32,
    end: u32,
}

impl Block {
    fn len(&self) -> u32 {
        self.end - self.start
    }
}

#[derive(Debug)]
struct ChunkAlloc<M> {
    chunk: Arc<Chunk<M>>,
    block: Block,
}

impl<M> ChunkAlloc<M> {
    fn memory(&self) -> &M {
        &self.chunk.memory
    }
}

impl<M> Drop for ChunkAlloc<M> {
    fn drop(&mut self) {
        let mut blocks = self.chunk.blocks.lock();
        if let Some(i) = blocks.iter().position(|x| x.start == self.block.start) {
            blocks.remove(i);
        }
    }
}

#[derive(Debug)]
struct HostMemory {
    memory: MappedDeviceMemory,
    buffer: Arc<UnsafeBuffer>,
    usage: BufferUsage,
}

trait ChunkMemory: Sized {
    fn oom_error() -> OomError;
    fn from_device_memory(device_memory: DeviceMemory) -> Result<Self>;
}

impl ChunkMemory for DeviceMemory {
    fn oom_error() -> OomError {
        OomError::OutOfDeviceMemory
    }
    fn from_device_memory(device_memory: Self) -> Result<Self> {
        Ok(device_memory)
    }
}

impl ChunkMemory for HostMemory {
    fn oom_error() -> OomError {
        OomError::OutOfHostMemory
    }
    fn from_device_memory(device_memory: DeviceMemory) -> Result<Self> {
        let usage = BufferUsage::transfer_src() | BufferUsage::transfer_dst();
        let buffer = UnsafeBuffer::new(
            device_memory.device().clone(),
            UnsafeBufferCreateInfo {
                size: device_memory.allocation_size(),
                usage,
                ..Default::default()
            },
        )?;
        unsafe { buffer.bind_memory(&device_memory, 0)? };
        let memory = MappedDeviceMemory::new(device_memory, 0..buffer.size())?;
        Ok(Self {
            memory,
            buffer,
            usage,
        })
    }
}

const CHUNK_ALIGN: u32 = 256;
const CHUNK_SIZE_MULTIPLE: usize = 256_000_000;

#[derive(Debug)]
struct Chunk<M> {
    memory: M,
    len: usize,
    blocks: Mutex<Vec<Block>>,
}

impl<M> Chunk<M> {
    fn new(device: Arc<Device>, len: usize, ids: &[u32]) -> Result<Arc<Self>>
    where
        M: ChunkMemory,
    {
        let len = CHUNK_SIZE_MULTIPLE * (1 + (len - 1) / CHUNK_SIZE_MULTIPLE);
        for id in ids {
            let result = DeviceMemory::allocate(
                device.clone(),
                MemoryAllocateInfo {
                    allocation_size: len as DeviceSize,
                    memory_type_index: *id,
                    ..Default::default()
                },
            );
            match result {
                Ok(device_memory) => {
                    let memory = M::from_device_memory(device_memory)?;
                    return Ok(Arc::new(Self {
                        memory,
                        len,
                        blocks: Mutex::default(),
                    }));
                }
                Err(DeviceMemoryAllocationError::OomError(e)) => continue,
                Err(e) => {
                    return Err(e.into());
                }
            }
        }
        Err(M::oom_error().into())
    }
    fn alloc(self: &Arc<Self>, len: u32) -> Option<Arc<ChunkAlloc<M>>> {
        if len as usize > self.len {
            return None;
        }
        let block_len = CHUNK_ALIGN * (1 + (len - 1) / CHUNK_ALIGN);
        let mut blocks = self.blocks.lock();
        let mut start = 0;
        for (i, block) in blocks.iter().enumerate() {
            if start + len <= block.start {
                let block = Block {
                    start,
                    end: start + block_len,
                };
                blocks.insert(i, block);
                return Some(Arc::new(ChunkAlloc {
                    chunk: self.clone(),
                    block,
                }));
            } else {
                start = block.end;
            }
        }
        if (start + len) as usize <= self.len {
            let block = Block {
                start,
                end: start + block_len,
            };
            blocks.push(block);
            Some(Arc::new(ChunkAlloc {
                chunk: self.clone(),
                block,
            }))
        } else {
            None
        }
    }
}

#[derive(Debug)]
struct BufferAllocator {
    device: Arc<Device>,
    host_ids: Vec<u32>,
    device_ids: Vec<u32>,
    host_chunks: Vec<Mutex<Weak<Chunk<HostMemory>>>>,
    device_chunks: Vec<Mutex<Weak<Chunk<DeviceMemory>>>>,
}

impl BufferAllocator {
    fn new(device: Arc<Device>) -> Result<Self> {
        let physical_device = device.physical_device();
        let mut max_host_chunks = 0;
        let mut max_device_chunks = 0;
        let mut host_ids = Vec::new();
        let mut device_ids = Vec::new();
        for memory_type in physical_device.memory_types() {
            let heap = memory_type.heap();
            if memory_type.is_host_visible() {
                max_host_chunks += (heap.size() / CHUNK_SIZE_MULTIPLE as u64) as usize;
                host_ids.push(memory_type.id());
            }
            if heap.is_device_local() {
                max_device_chunks += (heap.size() / CHUNK_SIZE_MULTIPLE as u64) as usize;
                device_ids.push(memory_type.id());
            }
        }
        // sort largest heap first
        host_ids.sort_by_key(|x| {
            -(physical_device.memory_type_by_id(*x).unwrap().heap().size() as i64)
        });
        device_ids.sort_by_key(|x| {
            -(physical_device.memory_type_by_id(*x).unwrap().heap().size() as i64)
        });
        let host_chunks = (0..max_host_chunks)
            .into_iter()
            .map(|_| Mutex::default())
            .collect();
        let device_chunks = (0..max_device_chunks)
            .into_iter()
            .map(|_| Mutex::default())
            .collect();
        Ok(Self {
            device,
            host_ids,
            device_ids,
            host_chunks,
            device_chunks,
        })
    }
    fn alloc_host(&self, len: u32) -> Result<Arc<HostBuffer>> {
        for chunk in self.host_chunks.iter() {
            let mut chunk = chunk.lock();
            if let Some(chunk) = Weak::upgrade(&chunk) {
                if let Some(alloc) = chunk.alloc(len) {
                    return HostBuffer::new(alloc, len);
                }
            } else {
                let new_chunk = Chunk::new(self.device.clone(), len as usize, &self.host_ids)?;
                let alloc = new_chunk.alloc(len).unwrap();
                *chunk = Arc::downgrade(&new_chunk);
                return HostBuffer::new(alloc, len);
            }
        }
        Err(OomError::OutOfHostMemory.into())
    }
    fn alloc_device(&self, len: u32) -> Result<Arc<DeviceBuffer>> {
        for chunk in self.device_chunks.iter() {
            let mut chunk = chunk.lock();
            if let Some(chunk) = Weak::upgrade(&chunk) {
                if let Some(alloc) = chunk.alloc(len) {
                    return DeviceBuffer::new(self.device.clone(), alloc, len);
                }
            } else {
                let new_chunk = Chunk::new(self.device.clone(), len as usize, &self.device_ids)?;
                let alloc = new_chunk.alloc(len).unwrap();
                *chunk = Arc::downgrade(&new_chunk);
                return DeviceBuffer::new(self.device.clone(), alloc, len);
            }
        }
        Err(OomError::OutOfHostMemory.into())
    }
}

#[derive(Debug)]
struct Upload {
    src: Arc<HostBuffer>,
    dst: Arc<DeviceBufferInner>,
}

impl Upload {
    fn barrier_key(&self) -> (usize, DeviceSize) {
        (self.dst.chunk_id(), self.dst.start())
    }
    fn barrier(&self) -> BufferMemoryBarrier {
        let source_stages = PipelineStages {
            transfer: true,
            compute_shader: true,
            ..Default::default()
        };
        let source_access = AccessFlags {
            transfer_read: true,
            transfer_write: true,
            shader_read: true,
            shader_write: true,
            ..Default::default()
        };
        let destination_stages = PipelineStages {
            transfer: true,
            ..Default::default()
        };
        let destination_access = AccessFlags {
            transfer_write: true,
            ..Default::default()
        };
        BufferMemoryBarrier {
            source_stages,
            source_access,
            destination_stages,
            destination_access,
            range: 0..self.dst.buffer.size(),
            ..BufferMemoryBarrier::buffer(self.dst.buffer.clone())
        }
    }
    fn copy_buffer_info(&self) -> CopyBufferInfo {
        CopyBufferInfo::buffers(self.src.clone(), self.dst.clone())
    }
}

#[derive(Debug)]
struct Download {
    src: Arc<DeviceBufferInner>,
    dst: Arc<HostBuffer>,
}

impl Download {
    fn barrier_key(&self) -> (usize, DeviceSize) {
        (self.src.chunk_id(), self.src.start())
    }
    fn barrier(&self) -> BufferMemoryBarrier {
        let source_stages = PipelineStages {
            transfer: true,
            compute_shader: true,
            ..Default::default()
        };
        let source_access = AccessFlags {
            transfer_write: true,
            transfer_read: true,
            shader_write: true,
            shader_read: true,
            ..Default::default()
        };
        let destination_stages = PipelineStages {
            transfer: true,
            ..Default::default()
        };
        let destination_access = AccessFlags {
            transfer_read: true,
            ..Default::default()
        };
        BufferMemoryBarrier {
            source_stages,
            source_access,
            destination_stages,
            destination_access,
            range: 0..self.src.buffer.size(),
            ..BufferMemoryBarrier::buffer(self.src.buffer.clone())
        }
    }
    fn copy_buffer_info(&self) -> CopyBufferInfo {
        CopyBufferInfo::buffers(self.src.clone(), self.dst.clone())
    }
}

#[derive(Debug)]
enum Op {
    Upload(Upload),
    Download(Download),
}

struct Frame {
    queue: Arc<Queue>,
    command_pool: UnsafeCommandPool,
    command_buffer: Option<(UnsafeCommandPoolAlloc, UnsafeCommandBuffer)>,
    semaphore: Semaphore,
    fence: Fence,
    ops: Vec<Op>,
    barriers: HashMap<(usize, DeviceSize), AccessFlags>,
}

impl Frame {
    fn new(queue: Arc<Queue>) -> Result<Self, anyhow::Error> {
        let device = queue.device();
        let command_pool_info = UnsafeCommandPoolCreateInfo {
            queue_family_index: queue.family().id(),
            transient: true,
            reset_command_buffer: false,
            ..UnsafeCommandPoolCreateInfo::default()
        };
        let command_pool = UnsafeCommandPool::new(device.clone(), command_pool_info)?;
        let semaphore = Semaphore::from_pool(device.clone())?;
        let fence = Fence::new(
            device.clone(),
            FenceCreateInfo {
                signaled: true,
                ..Default::default()
            },
        )?;
        let ops = Vec::new();
        let barriers = HashMap::default();
        Ok(Self {
            queue,
            command_pool,
            command_buffer: None,
            semaphore,
            fence,
            ops,
            barriers,
        })
    }
    fn poll(&mut self) -> Result<bool> {
        if self.fence.is_signaled()? {
            self.ops.clear();
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn submit<'a>(
        &mut self,
        ops: Vec<Op>,
        wait_semaphores: impl Iterator<Item = &'a Semaphore>,
    ) -> Result<()> {
        self.fence.wait(None).unwrap();
        self.fence.reset();
        self.command_buffer = None;
        let release_resources = false;
        unsafe {
            self.command_pool.reset(release_resources)?;
        }
        let command_pool_alloc = self
            .command_pool
            .allocate_command_buffers(Default::default())?
            .next()
            .unwrap();
        let mut cb_builder = unsafe {
            UnsafeCommandBufferBuilder::new(
                &command_pool_alloc,
                CommandBufferBeginInfo {
                    usage: CommandBufferUsage::OneTimeSubmit,
                    ..Default::default()
                },
            )?
        };
        for op in ops.iter() {
            match op {
                Op::Upload(upload) => {
                    let barrier = upload.barrier();
                    let prev_access = self
                        .barriers
                        .insert(upload.barrier_key(), barrier.destination_access)
                        .unwrap_or(AccessFlags::none());
                    if prev_access != AccessFlags::none() {
                        unsafe {
                            cb_builder.pipeline_barrier(&DependencyInfo {
                                buffer_memory_barriers: [barrier].into_iter().collect(),
                                ..Default::default()
                            })
                        }
                    }
                    unsafe {
                        cb_builder.copy_buffer(&upload.copy_buffer_info());
                    }
                }
                Op::Download(download) => unsafe {
                    let barrier = download.barrier();
                    let prev_access = self
                        .barriers
                        .insert(download.barrier_key(), barrier.destination_access)
                        .unwrap_or(barrier.destination_access);
                    if prev_access != barrier.destination_access {
                        unsafe {
                            cb_builder.pipeline_barrier(&DependencyInfo {
                                buffer_memory_barriers: [barrier].into_iter().collect(),
                                ..Default::default()
                            })
                        }
                    }
                    unsafe {
                        cb_builder.copy_buffer(&download.copy_buffer_info());
                    }
                },
            }
        }
        let command_buffer = cb_builder.build()?;
        let mut submit_builder = SubmitCommandBufferBuilder::new();
        for semaphore in wait_semaphores {
            unsafe {
                submit_builder.add_wait_semaphore(
                    semaphore,
                    PipelineStages {
                        bottom_of_pipe: true,
                        ..Default::default()
                    },
                );
            }
        }
        unsafe {
            submit_builder.add_command_buffer(&command_buffer);
        }
        self.semaphore = Semaphore::from_pool(self.queue.device().clone())?;
        unsafe {
            submit_builder.add_signal_semaphore(&self.semaphore);
            submit_builder.set_fence_signal(&self.fence);
        }
        submit_builder.submit(&self.queue)?;
        self.command_buffer
            .replace((command_pool_alloc, command_buffer));
        self.ops = ops;
        Ok(())
    }
}

struct Runner {
    queue: Arc<Queue>,
    op_receiver: Receiver<Op>,
    ready: VecDeque<Frame>,
    pending: VecDeque<Frame>,
    done: Arc<AtomicBool>,
    result: Arc<RwLock<Result<(), Arc<anyhow::Error>>>>,
}

impl Runner {
    fn new(
        queue: Arc<Queue>,
        op_receiver: Receiver<Op>,
        done: Arc<AtomicBool>,
        result: Arc<RwLock<Result<(), Arc<anyhow::Error>>>>,
    ) -> Result<Self, anyhow::Error> {
        let nframes = 3;
        let mut ready = VecDeque::with_capacity(nframes);
        for _ in 0..nframes {
            ready.push_back(Frame::new(queue.clone())?);
        }
        let pending = VecDeque::with_capacity(ready.len());
        Ok(Self {
            queue,
            op_receiver,
            ready,
            pending,
            done,
            result,
        })
    }
    fn run(&mut self) {
        let result = self.run_impl();
        if let Err(e) = result {
            *self.result.write() = Err(Arc::new(e));
        }
    }
    fn run_impl(&mut self) -> Result<()> {
        let mut last_submit = Instant::now();
        let n_ops = 1_000;
        let mut ops = Vec::with_capacity(n_ops);
        while !self.done.load(Ordering::Acquire) {
            if let Some(frame) = self.pending.front_mut() {
                if frame.poll()? {
                    self.ready.push_back(self.pending.pop_front().unwrap());
                }
            }
            ops.extend(
                self.op_receiver
                    .try_iter()
                    .take(n_ops.checked_sub(ops.len()).unwrap_or(0)),
            );
            if !ops.is_empty() {
                let pending0 =
                    self.pending.is_empty() && last_submit.elapsed() > Duration::from_millis(1);
                let pending1 = self.pending.len() == 1 && ops.len() >= n_ops;
                if pending0 || pending1 {
                    let mut frame = self.ready.pop_front().unwrap();
                    let wait_semaphores = self.pending.iter().map(|x| &x.semaphore);
                    let ops = core::mem::replace(&mut ops, Vec::with_capacity(n_ops));
                    frame.submit(ops, wait_semaphores)?;
                    self.pending.push_back(frame);
                    last_submit = Instant::now();
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if !self.done.load(Ordering::SeqCst) {
            let index = self.queue.device().physical_device().index();
            *self.result.write() = Err(Arc::new(anyhow!("Device({}) panicked!", index)));
        }
        self.queue.wait().unwrap();
    }
}
