//! Browser entry point (wasm32).
//!
//! The page copies the player's own Yuri's Revenge install into private
//! browser storage (OPFS). This entry point opens a canvas, requests the GPU
//! on WebGPU and reads those files back into memory, all asynchronously, then
//! starts the real app with that window and GPU and forwards events to it.

use std::sync::Arc;

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
use winit::window::{Window, WindowId};

use crate::app::App;
use crate::app::frontend::startup_options::RetailStartupOptions;
use crate::render::gpu::GpuContext;

/// OPFS folder and manifest the page writes (see web/index.html).
const STORE: &str = "ra2";
const MANIFEST: &str = "manifest.json";

/// Work that finishes asynchronously comes back through the event loop.
enum WebEvent {
    GpuReady(anyhow::Result<GpuContext>),
    FilesLoaded(Result<Vec<(String, Vec<u8>)>, String>),
}

struct WebApp {
    proxy: EventLoopProxy<WebEvent>,
    window: Option<Arc<Window>>,
    /// Held until the files arrive, then handed to the app.
    gpu: Option<GpuContext>,
    files_loaded: bool,
    app: Option<App>,
}

impl WebApp {
    /// Once both the GPU and the files are ready, start the real app.
    fn try_start_app(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.is_some() || !self.files_loaded {
            return;
        }
        let (Some(window), Some(gpu)) = (self.window.clone(), self.gpu.take()) else {
            return;
        };
        set_status("Starting the game…");
        let mut app = App::new(RetailStartupOptions::default()).with_prepared_surface(window, gpu);
        app.resumed(event_loop);
        self.app = Some(app);
        set_status("");
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

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WebEvent) {
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
                }
                self.gpu = Some(gpu);
                self.try_start_app(event_loop);
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
                self.try_start_app(event_loop);
            }
            WebEvent::FilesLoaded(Err(err)) => {
                log::error!("Could not read the game files: {err}");
                set_status(&format!("Could not read the game files: {err}"));
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(app) = &mut self.app {
            app.window_event(event_loop, id, event);
            return;
        }
        if let (WindowEvent::Resized(size), Some(gpu)) = (&event, &mut self.gpu) {
            gpu.resize(size.width, size.height);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(app) = &mut self.app {
            app.about_to_wait(event_loop);
        }
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(app) = &mut self.app {
            app.exiting(event_loop);
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
        files_loaded: false,
        app: None,
    };
    event_loop.spawn_app(app);
}
