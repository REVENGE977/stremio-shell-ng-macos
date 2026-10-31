pub fn gpu_video_processing_supported() -> bool {
    false
}

pub fn unified_memory_architecture() -> bool {
    cfg!(target_arch = "aarch64")
}
