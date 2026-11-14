use super::*;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, PartialEq)]
enum Call {
    Init,
    AcknowledgeQuit,
    Destroy(u64),
    Shutdown,
    ReleaseGraphics,
}

struct Runtime {
    overlay: Box<[usize; 69]>,
    system: Box<[usize; 48]>,
    events: HashMap<u64, VecDeque<u32>>,
    calls: Vec<Call>,
    next_handle: u64,
}

impl Runtime {
    fn new() -> Self {
        // Native FnTables are arrays of function addresses. Give every slot a
        // non-null address, with correctly typed functions for all called slots.
        let mut overlay = Box::new([unused as *const () as usize; 69]);
        overlay[1] = create_overlay as *const () as usize;
        overlay[3] = destroy_overlay as *const () as usize;
        overlay[11] = set_flag as *const () as usize;
        overlay[48] = poll_event as *const () as usize;
        let mut system = Box::new([unused as *const () as usize; 48]);
        system[47] = acknowledge_quit as *const () as usize;
        Self {
            overlay,
            system,
            events: HashMap::new(),
            calls: Vec::new(),
            next_handle: 0,
        }
    }
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::new());
}

unsafe extern "system" fn unused() {
    unreachable!("Unexpected OpenVR call");
}

unsafe extern "C" fn init(error: *mut i32, application: i32) -> u32 {
    assert_eq!(application, VR_APPLICATION_OVERLAY);
    unsafe { *error = 0 };
    RUNTIME.with_borrow_mut(|runtime| runtime.calls.push(Call::Init));
    1
}

unsafe extern "C" fn shutdown() {
    RUNTIME.with_borrow_mut(|runtime| {
        assert!(
            runtime.events.is_empty(),
            "Overlays must be destroyed first"
        );
        runtime.calls.push(Call::Shutdown);
    });
}

unsafe extern "C" fn installed() -> bool {
    true
}

unsafe extern "C" fn interface(name: *const c_char, error: *mut i32) -> *mut c_void {
    unsafe { *error = 0 };
    RUNTIME.with_borrow(|runtime| match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"FnTable:IVROverlay_028" => runtime.overlay.as_ptr().cast_mut().cast(),
        b"FnTable:IVRSystem_026" => runtime.system.as_ptr().cast_mut().cast(),
        _ => panic!("Unexpected OpenVR interface"),
    })
}

unsafe extern "system" fn create_overlay(
    _key: *const c_char,
    _name: *const c_char,
    handle: *mut u64,
) -> i32 {
    RUNTIME.with_borrow_mut(|runtime| {
        runtime.next_handle += 1;
        unsafe { *handle = runtime.next_handle };
        runtime.events.insert(runtime.next_handle, VecDeque::new());
    });
    0
}

unsafe extern "system" fn destroy_overlay(handle: u64) -> i32 {
    RUNTIME.with_borrow_mut(|runtime| {
        assert!(runtime.events.remove(&handle).is_some());
        runtime.calls.push(Call::Destroy(handle));
    });
    0
}

unsafe extern "system" fn set_flag(_handle: u64, _flag: i32, _enabled: bool) -> i32 {
    0
}

unsafe extern "system" fn poll_event(handle: u64, event: *mut OverlayEvent, size: u32) -> bool {
    assert_eq!(size as usize, std::mem::size_of::<OverlayEvent>());
    RUNTIME.with_borrow_mut(|runtime| {
        let Some(event_type) = runtime.events.get_mut(&handle).unwrap().pop_front() else {
            return false;
        };
        unsafe { (*event).event_type = event_type };
        true
    })
}

unsafe extern "system" fn acknowledge_quit() {
    RUNTIME.with_borrow_mut(|runtime| runtime.calls.push(Call::AcknowledgeQuit));
}

struct Graphics;
impl Drop for Graphics {
    fn drop(&mut self) {
        RUNTIME.with_borrow_mut(|runtime| runtime.calls.push(Call::ReleaseGraphics));
    }
}

fn api() -> Arc<OpenVrApi> {
    RUNTIME.with_borrow_mut(|runtime| *runtime = Runtime::new());
    Arc::new(OpenVrApi {
        #[cfg(windows)]
        _module: Default::default(),
        vr_init: init,
        vr_shutdown: shutdown,
        vr_is_runtime_installed: installed,
        _vr_is_hmd_present: installed,
        vr_get_generic_interface: interface,
    })
}

#[test]
fn idle_quit_is_acknowledged_before_teardown_and_allows_reconnect() {
    let api = api();
    for _ in 0..2 {
        RUNTIME.with_borrow_mut(|runtime| runtime.calls.clear());
        let session = api.init_overlay().unwrap();
        let caption = session.create_overlay("caption", "Caption").unwrap();
        let voice = session.create_overlay("voice", "Voice").unwrap();
        session.inner.graphics.borrow_mut().push(Box::new(Graphics));
        RUNTIME.with_borrow_mut(|runtime| {
            runtime
                .events
                .get_mut(&voice.handle)
                .unwrap()
                .extend([508, 700]);
        });

        // Neither surface has rendered or been shown. Quit can arrive on any
        // overlay, behind an ordinary event such as ImageLoaded (508).
        assert!(session.poll_quit());
        let handles = [voice.handle, caption.handle];
        drop(session);
        drop(voice);
        drop(caption);
        RUNTIME.with_borrow(|runtime| {
            assert_eq!(
                runtime.calls,
                [
                    Call::Init,
                    Call::AcknowledgeQuit,
                    Call::Destroy(handles[0]),
                    Call::Destroy(handles[1]),
                    Call::Shutdown,
                    Call::ReleaseGraphics,
                ]
            );
        });
    }
}

#[test]
fn ordinary_events_are_drained_from_all_live_overlays() {
    let api = api();
    let session = api.init_overlay().unwrap();
    let caption = session.create_overlay("caption", "Caption").unwrap();
    let voice = session.create_overlay("voice", "Voice").unwrap();
    RUNTIME.with_borrow_mut(|runtime| {
        for events in runtime.events.values_mut() {
            events.extend(std::iter::repeat_n(508, 256));
        }
    });
    assert!(!session.poll_quit());
    RUNTIME.with_borrow(|runtime| {
        assert!(runtime.events.values().all(VecDeque::is_empty));
        assert_eq!(runtime.calls, [Call::Init]);
    });

    drop(voice);
    assert!(!session.poll_quit()); // Must not poll the destroyed handle.
    drop(caption);
    assert!(!session.poll_quit());
}
