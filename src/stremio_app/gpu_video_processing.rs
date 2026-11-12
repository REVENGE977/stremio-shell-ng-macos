use crate::stremio_app::stremio_player::macos_gl;

/// Returns `true` when hardware-accelerated OpenGL is active and GPU video
/// post-processing shaders (e.g. Spline36 upscaling and debanding) can run on
/// the GPU without stalling playback.
pub fn gpu_video_processing_supported() -> bool {
    !macos_gl::is_software_renderer()
}
