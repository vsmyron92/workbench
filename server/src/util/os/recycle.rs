//! The Recycle Bin through the shell's `IFileOperation`, for a path `SHFileOperationW` does
//! not take (MAX_PATH characters or more; `fs::trash` sends shorter ones there). No UI:
//! where Windows would delete the item for good instead (the bin cannot take it), the
//! progress sink refuses. Its `PreDeleteItem` then comes without
//! `TSF_DELETE_RECYCLE_IF_POSSIBLE`, and an error from it cancels the delete and everything
//! after it ("the delete operation and all subsequent operations pending from the call to
//! IFileOperation are canceled"). Qt's `QFile::moveToTrash` relies on the same.

use std::ffi::c_void;
use std::io;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{E_FAIL, E_NOINTERFACE, E_POINTER, S_OK};
use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::UI::Shell::{FOF_ALLOWUNDO, FOF_NO_UI, FOFX_RECYCLEONDELETE, FileOperation, SHCreateItemFromParsingName, TSF_DELETE_RECYCLE_IF_POSSIBLE};
use windows_sys::core::{GUID, HRESULT, PCWSTR};

use super::win32::{Com, IUnknownVtbl, hr};

/// IID_IUnknown, {00000000-0000-0000-C000-000000000046}.
const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
/// IID_IFileOperation, {947AAB5F-0A5C-4C13-B4D6-4BF7836FC9F8}.
const IID_IFILEOPERATION: GUID = GUID::from_u128(0x947aab5f_0a5c_4c13_b4d6_4bf7836fc9f8);
/// IID_IFileOperationProgressSink, {04B0F1A7-9490-44BC-96E1-4296A31252E2}.
const IID_IFILEOPERATIONPROGRESSSINK: GUID = GUID::from_u128(0x04b0f1a7_9490_44bc_96e1_4296a31252e2);
/// IID_IShellItem, {43826D1E-E718-42EE-BC55-A1E261C37BFE}.
const IID_ISHELLITEM: GUID = GUID::from_u128(0x43826d1e_e718_42ee_bc55_a1e261c37bfe);

/// Why [`recycle`] did not move the item.
#[derive(Debug)]
pub enum NotRecycled {
    /// The shell cannot open the path as an item (the HRESULT): nothing happened.
    Unopened(HRESULT),
    /// Windows would have deleted the item for good; the sink stopped it, nothing happened.
    WouldDelete,
    /// Anything else, Windows deleting the item for good all the same included.
    Failed(io::Error),
}

/// Move the item at `path` (full, without the `\\?\` prefix, NUL-terminated) to the Recycle
/// Bin. On a thread of its own, with a single-threaded apartment as the shell's objects
/// want, so whatever apartment the caller's thread has stays as is.
pub fn recycle(path: Vec<u16>) -> Result<(), NotRecycled> {
    let worker = std::thread::Builder::new().name("recycle-bin".into()).spawn(move || {
        // SAFETY: COM is initialised on this new thread and uninitialised after every
        // interface pointer (the `Com` guards, dropped inside `delete`) is released.
        unsafe {
            hr(CoInitializeEx(ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32)).map_err(NotRecycled::Failed)?;
            let result = delete(&path);
            CoUninitialize();
            result
        }
    });
    worker.map_err(NotRecycled::Failed)?.join().map_err(|_| NotRecycled::Failed(io::Error::other("the Recycle Bin thread panicked")))?
}

/// # Safety
/// COM is initialised on this thread (single-threaded); `path` is NUL-terminated.
unsafe fn delete(path: &[u16]) -> Result<(), NotRecycled> {
    let mut p: *mut c_void = ptr::null_mut();
    // SAFETY: the FileOperation class and IFileOperation's id; `p` receives the interface on success.
    hr(unsafe { CoCreateInstance(&FileOperation, ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_IFILEOPERATION, &mut p) })
        .map_err(NotRecycled::Failed)?;
    let op = Com(p);
    let mut p: *mut c_void = ptr::null_mut();
    // SAFETY: `path` is NUL-terminated; no bind context; `p` receives an IShellItem on success.
    let opened = unsafe { SHCreateItemFromParsingName(path.as_ptr(), ptr::null_mut(), &IID_ISHELLITEM, &mut p) };
    if opened < 0 {
        return Err(NotRecycled::Unopened(opened));
    }
    let item = Com(p);
    let sink = Sink::new();
    // SAFETY: `op` is an IFileOperation, `item` an IShellItem and `sink` an
    // IFileOperationProgressSink, all live across the calls; `aborted` is a BOOL out-pointer.
    let (done, aborted) = unsafe {
        let v = op.vtbl::<IFileOperationVtbl>();
        hr((v.set_operation_flags)(op.0, FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_NO_UI)).map_err(NotRecycled::Failed)?;
        hr((v.delete_item)(op.0, item.0, sink.0)).map_err(NotRecycled::Failed)?;
        let done = (v.perform_operations)(op.0);
        let mut aborted = 0;
        (done, hr((v.get_any_operations_aborted)(op.0, &mut aborted)).is_ok() && aborted != 0)
    };
    // SAFETY: `sink` was made by `Sink::new` and holds a reference.
    let seen = unsafe { Sink::of(&sink) };
    if seen.refused.load(Ordering::Acquire) {
        return Err(NotRecycled::WouldDelete);
    }
    hr(done).map_err(NotRecycled::Failed)?;
    match seen.failed.load(Ordering::Acquire) {
        0 => {}
        e => return Err(NotRecycled::Failed(io::Error::from_raw_os_error(e))),
    }
    if aborted {
        return Err(NotRecycled::Failed(io::Error::other("the operation was cancelled")));
    }
    if seen.deleted.load(Ordering::Acquire) {
        return Err(NotRecycled::Failed(io::Error::other("Windows deleted it for good instead")));
    }
    if !seen.recycled.load(Ordering::Acquire) {
        return Err(NotRecycled::Failed(io::Error::other("Windows did not report it in the Recycle Bin")));
    }
    Ok(())
}

/// IFileOperation's vtable (shobjidl_core.h), in declaration order.
#[repr(C)]
#[allow(dead_code)]
struct IFileOperationVtbl {
    unknown: IUnknownVtbl,
    advise: *const c_void,
    unadvise: *const c_void,
    set_operation_flags: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
    set_progress_message: *const c_void,
    set_progress_dialog: *const c_void,
    set_properties: *const c_void,
    set_owner_window: *const c_void,
    apply_properties_to_item: *const c_void,
    apply_properties_to_items: *const c_void,
    rename_item: *const c_void,
    rename_items: *const c_void,
    move_item: *const c_void,
    move_items: *const c_void,
    copy_item: *const c_void,
    copy_items: *const c_void,
    delete_item: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> HRESULT,
    delete_items: *const c_void,
    new_item: *const c_void,
    perform_operations: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    get_any_operations_aborted: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT,
}

type This = *mut c_void;
type Item = *mut c_void;

/// IFileOperationProgressSink's vtable (shobjidl_core.h), in declaration order. Windows calls
/// every method, so each is a function of the declared arity (on 32-bit x86 the callee pops
/// the arguments).
#[repr(C)]
struct SinkVtbl {
    unknown: IUnknownVtbl,
    start_operations: unsafe extern "system" fn(This) -> HRESULT,
    finish_operations: unsafe extern "system" fn(This, HRESULT) -> HRESULT,
    pre_rename_item: unsafe extern "system" fn(This, u32, Item, PCWSTR) -> HRESULT,
    post_rename_item: unsafe extern "system" fn(This, u32, Item, PCWSTR, HRESULT, Item) -> HRESULT,
    pre_move_item: unsafe extern "system" fn(This, u32, Item, Item, PCWSTR) -> HRESULT,
    post_move_item: unsafe extern "system" fn(This, u32, Item, Item, PCWSTR, HRESULT, Item) -> HRESULT,
    pre_copy_item: unsafe extern "system" fn(This, u32, Item, Item, PCWSTR) -> HRESULT,
    post_copy_item: unsafe extern "system" fn(This, u32, Item, Item, PCWSTR, HRESULT, Item) -> HRESULT,
    pre_delete_item: unsafe extern "system" fn(This, u32, Item) -> HRESULT,
    post_delete_item: unsafe extern "system" fn(This, u32, Item, HRESULT, Item) -> HRESULT,
    pre_new_item: unsafe extern "system" fn(This, u32, Item, PCWSTR) -> HRESULT,
    post_new_item: unsafe extern "system" fn(This, u32, Item, PCWSTR, PCWSTR, u32, HRESULT, Item) -> HRESULT,
    update_progress: unsafe extern "system" fn(This, u32, u32) -> HRESULT,
    reset_timer: unsafe extern "system" fn(This) -> HRESULT,
    pause_timer: unsafe extern "system" fn(This) -> HRESULT,
    resume_timer: unsafe extern "system" fn(This) -> HRESULT,
}

static SINK_VTBL: SinkVtbl = SinkVtbl {
    unknown: IUnknownVtbl { query_interface, add_ref, release },
    start_operations: ok,
    finish_operations: ok_finish,
    pre_rename_item: ok_pre_named,
    post_rename_item: ok_post_renamed,
    pre_move_item: ok_pre_moved,
    post_move_item: ok_post_moved,
    pre_copy_item: ok_pre_moved,
    post_copy_item: ok_post_moved,
    pre_delete_item,
    post_delete_item,
    pre_new_item: ok_pre_named,
    post_new_item: ok_post_new,
    update_progress: ok_progress,
    reset_timer: ok,
    pause_timer: ok,
    resume_timer: ok,
};

/// An IFileOperationProgressSink that stops a delete Windows would not send to the Recycle
/// Bin, and notes what became of the item.
#[repr(C)]
struct Sink {
    vtbl: &'static SinkVtbl,
    refs: AtomicU32,
    /// A `PreDeleteItem` came without TSF_DELETE_RECYCLE_IF_POSSIBLE, and was refused.
    refused: AtomicBool,
    /// A `PostDeleteItem` named the item's new place in the Recycle Bin.
    recycled: AtomicBool,
    /// A `PostDeleteItem` reported a delete with no new place: gone for good.
    deleted: AtomicBool,
    /// The failure a `PostDeleteItem` reported, or 0.
    failed: AtomicI32,
}

impl Sink {
    /// A new sink, as an interface pointer that holds its one reference.
    fn new() -> Com {
        let sink = Box::new(Sink {
            vtbl: &SINK_VTBL,
            refs: AtomicU32::new(1),
            refused: AtomicBool::new(false),
            recycled: AtomicBool::new(false),
            deleted: AtomicBool::new(false),
            failed: AtomicI32::new(0),
        });
        Com(Box::into_raw(sink).cast())
    }

    /// # Safety
    /// `this` points to a live `Sink` (made by [`Sink::new`]).
    unsafe fn of(this: &Com) -> &Sink {
        // SAFETY: the caller's promise.
        unsafe { &*this.0.cast::<Sink>() }
    }
}

fn same_guid(a: &GUID, b: &GUID) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

// The sink's methods. Windows calls them with `this` a pointer `Sink::new` made, alive
// while Windows holds a reference.

unsafe extern "system" fn query_interface(this: This, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM passes a valid IID and a writable out-pointer.
    unsafe {
        if same_guid(&*iid, &IID_IUNKNOWN) || same_guid(&*iid, &IID_IFILEOPERATIONPROGRESSSINK) {
            add_ref(this);
            *out = this;
            S_OK
        } else {
            *out = ptr::null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn add_ref(this: This) -> u32 {
    // SAFETY: a live sink (see above).
    unsafe { &*this.cast::<Sink>() }.refs.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn release(this: This) -> u32 {
    // SAFETY: a live sink (see above); the last reference frees it, once.
    unsafe {
        let left = (*this.cast::<Sink>()).refs.fetch_sub(1, Ordering::Release) - 1;
        if left == 0 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(this.cast::<Sink>()));
        }
        left
    }
}

/// Go on with a delete that sends the item to the Recycle Bin; refuse (and so cancel
/// everything) one that would delete it for good.
unsafe extern "system" fn pre_delete_item(this: This, flags: u32, _item: Item) -> HRESULT {
    if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE as u32 != 0 {
        return S_OK;
    }
    // SAFETY: a live sink (see above).
    unsafe { &*this.cast::<Sink>() }.refused.store(true, Ordering::Release);
    E_FAIL
}

unsafe extern "system" fn post_delete_item(this: This, _flags: u32, _item: Item, result: HRESULT, in_bin: Item) -> HRESULT {
    // SAFETY: a live sink (see above).
    let sink = unsafe { &*this.cast::<Sink>() };
    if result < 0 {
        sink.failed.store(result, Ordering::Release);
    } else if in_bin.is_null() {
        // "If the item was fully deleted, this value is NULL."
        sink.deleted.store(true, Ordering::Release);
    } else {
        sink.recycled.store(true, Ordering::Release);
    }
    S_OK
}

unsafe extern "system" fn ok(_: This) -> HRESULT {
    S_OK
}

unsafe extern "system" fn ok_finish(_: This, _: HRESULT) -> HRESULT {
    S_OK
}

unsafe extern "system" fn ok_pre_named(_: This, _: u32, _: Item, _: PCWSTR) -> HRESULT {
    S_OK
}

unsafe extern "system" fn ok_post_renamed(_: This, _: u32, _: Item, _: PCWSTR, _: HRESULT, _: Item) -> HRESULT {
    S_OK
}

unsafe extern "system" fn ok_pre_moved(_: This, _: u32, _: Item, _: Item, _: PCWSTR) -> HRESULT {
    S_OK
}

unsafe extern "system" fn ok_post_moved(_: This, _: u32, _: Item, _: Item, _: PCWSTR, _: HRESULT, _: Item) -> HRESULT {
    S_OK
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn ok_post_new(_: This, _: u32, _: Item, _: PCWSTR, _: PCWSTR, _: u32, _: HRESULT, _: Item) -> HRESULT {
    S_OK
}

unsafe extern "system" fn ok_progress(_: This, _: u32, _: u32) -> HRESULT {
    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sink as Windows sees it: its interfaces, and what it answers before and after a
    /// delete. The Recycle Bin itself is `fs`'s tests' (on CI).
    #[test]
    fn the_sink_refuses_what_would_skip_the_bin() {
        let sink = Sink::new();
        // SAFETY: `sink` is a live Sink, called through its own vtable as Windows calls it.
        unsafe {
            let v = sink.vtbl::<SinkVtbl>();
            let mut p: *mut c_void = ptr::null_mut();
            assert_eq!((v.unknown.query_interface)(sink.0, &IID_IFILEOPERATIONPROGRESSSINK, &mut p), S_OK);
            assert_eq!(p, sink.0);
            let again = Com(p);
            assert_eq!(Sink::of(&sink).refs.load(Ordering::Relaxed), 2);
            drop(again);
            assert_eq!((v.unknown.query_interface)(sink.0, &IID_ISHELLITEM, &mut p), E_NOINTERFACE);
            assert!(p.is_null());

            assert_eq!((v.pre_delete_item)(sink.0, TSF_DELETE_RECYCLE_IF_POSSIBLE as u32, ptr::null_mut()), S_OK);
            assert!(!Sink::of(&sink).refused.load(Ordering::Relaxed));
            assert_eq!((v.pre_delete_item)(sink.0, 0, ptr::null_mut()), E_FAIL);
            assert!(Sink::of(&sink).refused.load(Ordering::Relaxed));

            let mut in_bin = 0u8;
            assert_eq!((v.post_delete_item)(sink.0, 0, ptr::null_mut(), S_OK, (&raw mut in_bin).cast()), S_OK);
            assert!(Sink::of(&sink).recycled.load(Ordering::Relaxed));
            assert!(!Sink::of(&sink).deleted.load(Ordering::Relaxed));
            assert_eq!((v.post_delete_item)(sink.0, 0, ptr::null_mut(), S_OK, ptr::null_mut()), S_OK);
            assert!(Sink::of(&sink).deleted.load(Ordering::Relaxed));
            assert_eq!((v.post_delete_item)(sink.0, 0, ptr::null_mut(), E_FAIL, ptr::null_mut()), S_OK);
            assert_eq!(Sink::of(&sink).failed.load(Ordering::Relaxed), E_FAIL);
        }
    }
}
