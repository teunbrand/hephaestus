//! Smoke test: build a Vello renderer, draw nothing, render a solid-background
//! frame, assert the center pixel matches the background. Validates wgpu init,
//! Vello pipeline, readback alignment, and PNG-shaped buffer layout.
//!
//! The size guard is here too, since it needs the same working adapter.

use hephaestus::backend::vello::VelloRenderer;
use hephaestus::color::rgb8;
use hephaestus::Renderer;

#[test]
fn solid_background_64x64() {
    let mut r = VelloRenderer::new().expect("vello renderer init");
    let w = 64u32;
    let h = 64u32;
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let bg = rgb8(255, 64, 32);
    r.render_to_buffer(w, h, bg, &mut buf).expect("render");

    // Sample the center pixel.
    let cx = w / 2;
    let cy = h / 2;
    let i = ((cy * w + cx) * 4) as usize;
    let (r8, g8, b8, a8) = (buf[i], buf[i + 1], buf[i + 2], buf[i + 3]);

    // Allow ±2 for any minor color-conversion drift.
    let approx = |v: u8, target: u8| (v as i16 - target as i16).abs() <= 2;
    assert!(approx(r8, 255), "red channel: {r8}");
    assert!(approx(g8, 64), "green channel: {g8}");
    assert!(approx(b8, 32), "blue channel: {b8}");
    assert_eq!(a8, 255, "alpha");
}

#[test]
fn a_frame_past_the_device_limit_is_an_error_rather_than_a_panic() {
    // The device is opened with wgpu's defaults, so its texture ceiling is
    // known: 8192. Past it the target texture would fail wgpu validation,
    // which panics, so the frame is checked before anything is allocated.
    let (device, queue) = make_device();
    let limit = device.limits().max_texture_dimension_2d;
    let mut r = VelloRenderer::with_device(&device, &queue).expect("vello renderer init");

    let (w, h) = (limit + 1, 8);
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
    let err = r
        .render_to_buffer(w, h, rgb8(0, 0, 0), &mut buf)
        .expect_err("a frame past the device limit cannot be rendered");
    assert!(
        err.to_string().contains(&limit.to_string()),
        "the error names the limit: {err}"
    );
}

/// An isolated wgpu device, opened with the limits wgpu itself defaults to
/// rather than the ones this crate asks for.
fn make_device() -> (hephaestus::wgpu::Device, hephaestus::wgpu::Queue) {
    use hephaestus::wgpu;
    pollster::block_on(async {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::PRIMARY;
        let instance = wgpu::Instance::new(desc);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .expect("adapter");
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("smoke.device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::default(),
            })
            .await
            .expect("device")
    })
}
