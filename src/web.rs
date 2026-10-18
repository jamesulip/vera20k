//! Browser entry point (wasm32).
//!
//! The page copies the player's own Yuri's Revenge install into private
//! browser storage (OPFS). This entry point opens a canvas, brings up the real
//! GPU context on WebGPU, reads those files back into memory and builds the
//! real `AssetManager` from them. As proof that game data loads, it draws the
//! country flags from the archives; the app orchestrator is not wired in yet.

use std::path::Path;
use std::sync::Arc;

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
use winit::window::{Window, WindowId};

use crate::render::gpu::GpuContext;
use vera20k_core::assets::asset_manager::{AssetManager, MediaArchiveMode};
use vera20k_core::assets::pcx_file::PcxFile;

/// Canvas fill behind the game art: a dark slate, distinct from the page.
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.08,
    g: 0.10,
    b: 0.14,
    a: 1.0,
};

/// OPFS folder and manifest the page writes (see web/index.html).
const STORE: &str = "ra2";
const MANIFEST: &str = "manifest.json";

/// The install directory as the engine names it; files resolve from memory.
const WEB_RA2_DIR: &str = "/ra2";

/// Country flags to draw, in sidebar order. Missing ones are skipped.
const FLAGS: &[&str] = &[
    "usai.pcx", "kori.pcx", "frai.pcx", "geri.pcx", "gbri.pcx", "lati.pcx", "arbi.pcx",
    "cubi.pcx", "rusi.pcx", "yrii.pcx",
];

/// Nearest-neighbour enlargement so the small flags are easy to see.
const FLAG_SCALE: u32 = 4;
const FLAG_GAP: u32 = 24;

/// Work that finishes asynchronously comes back through the event loop.
enum WebEvent {
    GpuReady(anyhow::Result<GpuContext>),
    FilesLoaded(Result<Vec<(String, Vec<u8>)>, String>),
}

/// A decoded image already uploaded in the surface's format.
struct Picture {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
}

struct WebApp {
    proxy: EventLoopProxy<WebEvent>,
    window: Option<Arc<Window>>,
    gpu: Option<GpuContext>,
    assets: Option<AssetManager>,
    files_loaded: bool,
    pictures: Vec<Picture>,
}

impl WebApp {
    /// Once both the GPU and the files are ready, build the asset manager and
    /// decode the flags.
    fn try_build_assets(&mut self) {
        if self.assets.is_some() || !self.files_loaded {
            return;
        }
        let Some(gpu) = &self.gpu else {
            return;
        };
        set_status("Opening game archives…");
        let assets = match AssetManager::new(Path::new(WEB_RA2_DIR), MediaArchiveMode::STOCK_DIGITAL)
        {
            Ok(assets) => assets,
            Err(err) => {
                log::error!("Could not load the game archives: {err}");
                set_status(&format!("Could not load the game archives: {err}"));
                return;
            }
        };
        self.pictures = FLAGS
            .iter()
            .filter_map(|name| {
                let bytes = assets.get_ref(name)?;
                match PcxFile::from_bytes(bytes) {
                    Ok(pcx) => upload_picture(gpu, &pcx, FLAG_SCALE),
                    Err(err) => {
                        log::warn!("Could not decode {name}: {err}");
                        None
                    }
                }
            })
            .collect();
        log::info!("Drew {} country flags from the game archives", self.pictures.len());
        set_status(&format!(
            "Game archives loaded in the browser: {} country flags from your install",
            self.pictures.len()
        ));
        self.assets = Some(assets);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler<WebEvent> for WebApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("VERA20k")
            .with_inner_size(LogicalSize::new(1024.0, 768.0))
            .with_append(true);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                log::error!("Could not create the browser canvas: {err}");
                return;
            }
        };
        self.window = Some(window.clone());

        // Browsers cannot block on a promise, so both requests run as tasks.
        let proxy = self.proxy.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = GpuContext::new_async(window).await;
            let _ = proxy.send_event(WebEvent::GpuReady(result));
        });
        let proxy = self.proxy.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = read_game_files().await.map_err(describe_js_error);
            let _ = proxy.send_event(WebEvent::FilesLoaded(result));
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: WebEvent) {
        match event {
            WebEvent::GpuReady(Ok(mut gpu)) => {
                let info = gpu.capture_adapter_observation();
                log::info!("WebGPU ready: {} ({:?})", info.name, info.backend);
                if let Some(window) = &self.window {
                    // The canvas may have been laid out while the GPU request
                    // was pending; that resize event found no surface to update.
                    let size = window.inner_size();
                    gpu.resize(size.width, size.height);
                    log::info!("Canvas surface: {}x{}", gpu.config.width, gpu.config.height);
                    window.request_redraw();
                }
                self.gpu = Some(gpu);
                self.try_build_assets();
            }
            WebEvent::GpuReady(Err(err)) => {
                log::error!("WebGPU initialization failed: {err:#}");
                set_status(&format!("WebGPU initialization failed: {err:#}"));
            }
            WebEvent::FilesLoaded(Ok(files)) => {
                let total: usize = files.iter().map(|(_, bytes)| bytes.len()).sum();
                log::info!(
                    "Read {} game files ({} MB) from browser storage",
                    files.len(),
                    total / 1_000_000
                );
                vera20k_core::assets::web_files::install(files);
                self.files_loaded = true;
                self.try_build_assets();
            }
            WebEvent::FilesLoaded(Err(err)) => {
                log::error!("Could not read the game files: {err}");
                set_status(&format!("Could not read the game files: {err}"));
            }
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                    log::info!("Canvas surface: {}x{}", gpu.config.width, gpu.config.height);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(gpu) = &self.gpu {
                    draw_frame(gpu, &self.pictures);
                }
            }
            _ => {}
        }
    }
}

/// Read every file the page stored, in manifest order.
async fn read_game_files() -> Result<Vec<(String, Vec<u8>)>, JsValue> {
    let window = web_sys::window().ok_or("no browser window")?;
    let root: web_sys::FileSystemDirectoryHandle =
        JsFuture::from(window.navigator().storage().get_directory())
            .await?
            .dyn_into()?;
    let store: web_sys::FileSystemDirectoryHandle =
        JsFuture::from(root.get_directory_handle(STORE)).await?.dyn_into()?;

    let manifest = read_file(&store, MANIFEST).await?;
    let manifest = String::from_utf8(manifest).map_err(|err| err.to_string())?;
    let entries: js_sys::Array = js_sys::JSON::parse(&manifest)?.dyn_into()?;
    let names: Vec<String> = entries
        .iter()
        .filter_map(|entry| js_sys::Reflect::get(&entry, &"name".into()).ok()?.as_string())
        .collect();

    let mut files = Vec::with_capacity(names.len());
    for (index, name) in names.iter().enumerate() {
        set_status(&format!("Reading game files {}/{}: {name}", index + 1, names.len()));
        let bytes = read_file(&store, name).await?;
        files.push((name.clone(), bytes));
    }
    Ok(files)
}

async fn read_file(store: &web_sys::FileSystemDirectoryHandle, name: &str) -> Result<Vec<u8>, JsValue> {
    let handle: web_sys::FileSystemFileHandle =
        JsFuture::from(store.get_file_handle(name)).await?.dyn_into()?;
    let file: web_sys::File = JsFuture::from(handle.get_file()).await?.dyn_into()?;
    let buffer = JsFuture::from(file.array_buffer()).await?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

fn describe_js_error(err: JsValue) -> String {
    err.as_string()
        .or_else(|| {
            err.dyn_ref::<js_sys::Error>()
                .map(|err| String::from(err.message()))
        })
        .unwrap_or_else(|| format!("{err:?}"))
}

/// Show a line of progress in the page's status bar.
fn set_status(text: &str) {
    if let Some(element) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("status"))
    {
        element.set_text_content(Some(text));
    }
}

/// Decode `pcx` with integer enlargement into a texture in the surface format,
/// so it can be copied straight onto the frame.
fn upload_picture(gpu: &GpuContext, pcx: &PcxFile, scale: u32) -> Option<Picture> {
    let swap_red_blue = match gpu.surface_format {
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
        other => {
            log::warn!("Cannot copy pictures onto a {other:?} surface");
            return None;
        }
    };
    let (src_w, src_h) = (u32::from(pcx.width), u32::from(pcx.height));
    let (width, height) = (src_w * scale, src_h * scale);
    let rgba = pcx.to_rgba(None);
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let at = (((y / scale) * src_w + x / scale) * 4) as usize;
            let [r, g, b, a] = [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]];
            if swap_red_blue {
                pixels.extend_from_slice(&[b, g, r, a]);
            } else {
                pixels.extend_from_slice(&[r, g, b, a]);
            }
        }
    }
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("web picture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: gpu.surface_format,
        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        texture.as_image_copy(),
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        size,
    );
    Some(Picture {
        texture,
        width,
        height,
    })
}

/// Clear the canvas, then lay the pictures out in centred rows.
fn draw_frame(gpu: &GpuContext, pictures: &[Picture]) {
    let frame = match gpu.surface.get_current_texture() {
        Ok(frame) => frame,
        Err(err) => {
            log::warn!("Skipped a frame: {err}");
            return;
        }
    };
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("web frame"),
        });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("web clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    let (surface_w, surface_h) = (gpu.config.width, gpu.config.height);
    let mut rows: Vec<Vec<&Picture>> = vec![Vec::new()];
    let mut row_w = 0;
    for picture in pictures {
        if picture.width + FLAG_GAP > surface_w || picture.height + FLAG_GAP > surface_h {
            continue;
        }
        if row_w + picture.width + FLAG_GAP > surface_w && !rows.last().unwrap().is_empty() {
            rows.push(Vec::new());
            row_w = 0;
        }
        row_w += picture.width + FLAG_GAP;
        rows.last_mut().unwrap().push(picture);
    }
    let row_heights: Vec<u32> = rows
        .iter()
        .map(|row| row.iter().map(|p| p.height).max().unwrap_or(0))
        .collect();
    let total_h: u32 = row_heights.iter().map(|h| h + FLAG_GAP).sum();
    let mut y = surface_h.saturating_sub(total_h) / 2;
    for (row, row_h) in rows.iter().zip(&row_heights) {
        let width: u32 = row.iter().map(|p| p.width + FLAG_GAP).sum::<u32>();
        let mut x = surface_w.saturating_sub(width) / 2 + FLAG_GAP / 2;
        for picture in row {
            if y + picture.height <= surface_h {
                encoder.copy_texture_to_texture(
                    picture.texture.as_image_copy(),
                    wgpu::TexelCopyTextureInfo {
                        texture: &frame.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x, y, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: picture.width,
                        height: picture.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            x += picture.width + FLAG_GAP;
        }
        y += row_h + FLAG_GAP;
    }

    gpu.queue.submit([encoder.finish()]);
    frame.present();
}

/// Called from `main` when the wasm module loads.
pub fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    log::info!("VERA20k web build starting");

    let event_loop = match EventLoop::<WebEvent>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            log::error!("Could not create the event loop: {err}");
            return;
        }
    };
    let app = WebApp {
        proxy: event_loop.create_proxy(),
        window: None,
        gpu: None,
        assets: None,
        files_loaded: false,
        pictures: Vec::new(),
    };
    event_loop.spawn_app(app);
}
