//! A private, surfaceless GL context on the same physical GPU as the Vulkan UI.
use eframe::glow::{self, HasContext};
use glutin::{
    api::egl::{context::PossiblyCurrentContext, device::Device, display::Display},
    config::{Api, ConfigSurfaceTypes, ConfigTemplateBuilder},
    context::{ContextApi, ContextAttributesBuilder, GlProfile, Version},
    prelude::*,
};
use std::ffi::{CStr, c_char, c_void};

pub const OPAQUE_FD: u32 = 0x9586;
pub const TRANSFER_SRC: u32 = 0x9592;

// glow does not expose EXT_external_objects. Load only the standard functions
// needed for Vulkan interop, after checking the advertised extensions.
pub struct ExternalObjects {
    pub create_memory: unsafe extern "system" fn(i32, *mut u32),
    pub delete_memory: unsafe extern "system" fn(i32, *const u32),
    pub memory_parameter: unsafe extern "system" fn(u32, u32, *const i32),
    pub import_memory: unsafe extern "system" fn(u32, u64, u32, i32),
    pub texture_storage: unsafe extern "system" fn(u32, i32, u32, i32, i32, u32, u64),
    pub create_semaphores: unsafe extern "system" fn(i32, *mut u32),
    pub delete_semaphores: unsafe extern "system" fn(i32, *const u32),
    pub import_semaphore: unsafe extern "system" fn(u32, u32, i32),
    pub wait: unsafe extern "system" fn(u32, u32, *const u32, u32, *const u32, *const u32),
    pub signal: unsafe extern "system" fn(u32, u32, *const u32, u32, *const u32, *const u32),
}

pub struct GlContext {
    pub gl: glow::Context,
    pub external: ExternalObjects,
    pub display: Display,
    _context: CurrentContext,
}

struct CurrentContext(PossiblyCurrentContext);

impl Drop for CurrentContext {
    fn drop(&mut self) {
        let _ = self.0.make_not_current_in_place();
    }
}

impl GlContext {
    pub fn new(device_uuid: [u8; 16], driver_uuid: [u8; 16]) -> Result<Box<Self>, String> {
        // Device UUIDs are the API-defined identity for cross-API memory sharing.
        // Never silently choose a second GPU or a software renderer.
        for device in Device::query_devices().map_err(|e| e.to_string())? {
            match Self::on_device(&device, device_uuid, driver_uuid) {
                Ok(Some(context)) => return Ok(context),
                Ok(None) => {}
                Err(error) => log::debug!("Video EGL device unavailable: {error}"),
            }
        }
        Err("No EGL device supports Vulkan video sharing on the UI GPU; check the installed graphics drivers and EGL driver overrides".into())
    }

    fn on_device(
        device: &Device,
        device_uuid: [u8; 16],
        driver_uuid: [u8; 16],
    ) -> Result<Option<Box<Self>>, String> {
        let display = unsafe { Display::with_device(device, None) }.map_err(|e| e.to_string())?;
        let template = ConfigTemplateBuilder::new()
            .with_api(Api::OPENGL)
            .with_surface_type(ConfigSurfaceTypes::PBUFFER)
            .with_depth_size(0)
            .with_stencil_size(0)
            .build();
        let config = unsafe { display.find_configs(template) }
            .map_err(|e| e.to_string())?
            .next()
            .ok_or("No offscreen OpenGL configuration")?;
        let attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
            .with_profile(GlProfile::Core)
            .build(None);
        let context = unsafe { display.create_context(&config, &attributes) }
            .and_then(|context| context.make_current_surfaceless())
            .map_err(|e| e.to_string())?;
        let context = CurrentContext(context);
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name))
        };
        for extension in ["GL_EXT_memory_object_fd", "GL_EXT_semaphore_fd"] {
            if !gl.supported_extensions().contains(extension) {
                return Ok(None);
            }
        }
        macro_rules! load {
            ($name:literal, $signature:ty) => {{
                let pointer = display.get_proc_address($name);
                if pointer.is_null() {
                    return Err(format!("Missing GL entry point {:?}", $name));
                }
                // SAFETY: The extension is advertised and the signature is from
                // the Khronos EXT_external_objects / _fd specifications.
                unsafe { std::mem::transmute::<*const c_void, $signature>(pointer) }
            }};
        }
        let get_uuid = load!(
            c"glGetUnsignedBytei_vEXT",
            unsafe extern "system" fn(u32, u32, *mut u8)
        );
        let matches = (0..unsafe { gl.get_parameter_i32(0x9596) }).any(|index| {
            let mut uuid = [0; 16];
            unsafe { get_uuid(0x9597, index as u32, uuid.as_mut_ptr()) };
            uuid == device_uuid
        });
        if !matches {
            return Ok(None);
        }
        let get_driver = load!(
            c"glGetUnsignedBytevEXT",
            unsafe extern "system" fn(u32, *mut u8)
        );
        let mut driver = [0; 16];
        unsafe { get_driver(0x9598, driver.as_mut_ptr()) };
        if driver != driver_uuid {
            return Ok(None);
        }
        let external = ExternalObjects {
            create_memory: load!(
                c"glCreateMemoryObjectsEXT",
                unsafe extern "system" fn(i32, *mut u32)
            ),
            delete_memory: load!(
                c"glDeleteMemoryObjectsEXT",
                unsafe extern "system" fn(i32, *const u32)
            ),
            memory_parameter: load!(
                c"glMemoryObjectParameterivEXT",
                unsafe extern "system" fn(u32, u32, *const i32)
            ),
            import_memory: load!(
                c"glImportMemoryFdEXT",
                unsafe extern "system" fn(u32, u64, u32, i32)
            ),
            texture_storage: load!(
                c"glTexStorageMem2DEXT",
                unsafe extern "system" fn(u32, i32, u32, i32, i32, u32, u64)
            ),
            create_semaphores: load!(
                c"glGenSemaphoresEXT",
                unsafe extern "system" fn(i32, *mut u32)
            ),
            delete_semaphores: load!(
                c"glDeleteSemaphoresEXT",
                unsafe extern "system" fn(i32, *const u32)
            ),
            import_semaphore: load!(
                c"glImportSemaphoreFdEXT",
                unsafe extern "system" fn(u32, u32, i32)
            ),
            wait: load!(
                c"glWaitSemaphoreEXT",
                unsafe extern "system" fn(u32, u32, *const u32, u32, *const u32, *const u32)
            ),
            signal: load!(
                c"glSignalSemaphoreEXT",
                unsafe extern "system" fn(u32, u32, *const u32, u32, *const u32, *const u32)
            ),
        };
        log::info!("Media GPU context: {}", unsafe {
            gl.get_parameter_string(glow::RENDERER)
        });
        Ok(Some(Box::new(Self {
            gl,
            external,
            display,
            _context: context,
        })))
    }

    pub fn check(&self) -> Result<(), String> {
        let error = unsafe { self.gl.get_error() };
        if error == glow::NO_ERROR {
            Ok(())
        } else {
            Err(format!("Video GL error: {error:#x}"))
        }
    }
}

pub unsafe extern "C" fn get_proc_address(data: *mut c_void, name: *const c_char) -> *mut c_void {
    let display = unsafe { &*(data as *const Display) };
    display
        .get_proc_address(unsafe { CStr::from_ptr(name) })
        .cast_mut()
}
