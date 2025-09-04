//! Minimal WinTab (wintab32.dll) FFI surface.
//!
//! Provides dynamic loading of the ANSI WinTab entry points required for this utility:
//! * WTInfoA  – retrieve default LOGCONTEXT template.
//! * WTOpenA  – open a tablet context bound to a window.
//! * WTSetA / WTGetA – apply or query context state.
//! * WTClose  – close an existing context.
//!
//! All function resolution is lazy and cached (OnceCell). Public helpers wrap the raw calls with
//! anyhow::Result for ergonomic error propagation. No global mutable state beyond the cached
//! function pointers is introduced.

use anyhow::{Result, anyhow};
use once_cell::sync::OnceCell;
use std::mem::zeroed;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA};
use windows::core::PCSTR;

/// Wintab context handle (opaque pointer value supplied by driver).
#[allow(clippy::upper_case_acronyms)]
pub type HCTX = isize;
/// Packet bitfield type.
#[allow(clippy::upper_case_acronyms)]
pub type WTPKT = u32;
/// 16.16 fixed point type used by WinTab (treated opaquely here).
pub type FIX32 = i32;

// WTInfo categories (subset)
/// WTInfo category for the default context template.
pub const WTI_DEFCONTEXT: u32 = 3; // default context

// Context option flags (subset)
#[allow(dead_code)]
pub const CXO_SYSTEM: u32 = 0x0001;
#[allow(dead_code)]
pub const CXO_PEN: u32 = 0x0002;
pub const CXO_MESSAGES: u32 = 0x0004; // we want window messages

/// Rust representation of the WinTab LOGCONTEXTA structure (layout sensitive).
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(non_snake_case)]
#[allow(clippy::upper_case_acronyms)]
pub struct LOGCONTEXTA {
    pub lcName: [u8; 40], // context name (null-terminated)
    pub lcOptions: u32,
    pub lcStatus: u32,
    pub lcLocks: u32,
    pub lcMsgBase: u32,
    pub lcDevice: u32,
    pub lcPktRate: u32,
    pub lcPktData: WTPKT,
    pub lcPktMode: WTPKT,
    pub lcMoveMask: WTPKT,
    pub lcBtnDnMask: u32,
    pub lcBtnUpMask: u32,
    pub lcInOrgX: i32,
    pub lcInOrgY: i32,
    pub lcInOrgZ: i32,
    pub lcInExtX: i32,
    pub lcInExtY: i32,
    pub lcInExtZ: i32,
    pub lcOutOrgX: i32,
    pub lcOutOrgY: i32,
    pub lcOutOrgZ: i32,
    pub lcOutExtX: i32,
    pub lcOutExtY: i32,
    pub lcOutExtZ: i32,
    pub lcSensX: FIX32,
    pub lcSensY: FIX32,
    pub lcSensZ: FIX32,
    pub lcSysMode: i32, // BOOL in header, keep 4 bytes
    pub lcSysOrgX: i32,
    pub lcSysOrgY: i32,
    pub lcSysExtX: i32,
    pub lcSysExtY: i32,
    pub lcSysSensX: FIX32,
    pub lcSysSensY: FIX32,
    pub lcSysSensZ: FIX32,
}

impl Default for LOGCONTEXTA {
    fn default() -> Self {
        unsafe { zeroed() }
    }
}

type PfnWtInfoA = unsafe extern "system" fn(u32, u32, *mut core::ffi::c_void) -> u32;
type PfnWtOpenA = unsafe extern "system" fn(HWND, *const LOGCONTEXTA, i32) -> HCTX;
type PfnWtClose = unsafe extern "system" fn(HCTX) -> i32;
type PfnWtGetA = unsafe extern "system" fn(HCTX, *mut LOGCONTEXTA) -> i32;
type PfnWtSetA = unsafe extern "system" fn(HCTX, *const LOGCONTEXTA) -> i32;

#[allow(dead_code)]
struct WintabFns {
    info: PfnWtInfoA,
    open: PfnWtOpenA,
    close: PfnWtClose,
    get: PfnWtGetA,
    set: PfnWtSetA,
}
static FNS: OnceCell<Option<WintabFns>> = OnceCell::new();

/// Attempt to load (or retrieve cached) function pointers for the WinTab DLL.
#[allow(
    clippy::manual_c_str_literals,
    clippy::question_mark,
    clippy::missing_transmute_annotations
)]
fn load_wintab() -> Option<&'static WintabFns> {
    FNS.get_or_init(|| unsafe {
        let name = PCSTR(b"wintab32.dll\0".as_ptr());
        let h = GetModuleHandleA(name)
            .ok()
            .or_else(|| LoadLibraryA(name).ok());
        let hmod = h?;
        let sym = |s: &str| {
            let mut v = Vec::with_capacity(s.len() + 1);
            v.extend_from_slice(s.as_bytes());
            v.push(0);
            GetProcAddress(hmod, PCSTR(v.as_ptr()))
        };
        macro_rules! need {
            ($n:literal) => {
                match sym($n) {
                    Some(p) => p,
                    None => return None,
                }
            };
        }
        let info = need!("WTInfoA");
        let open = need!("WTOpenA");
        let close = need!("WTClose");
        let get = need!("WTGetA");
        let set = need!("WTSetA");
        Some(WintabFns {
            info: std::mem::transmute::<_, PfnWtInfoA>(info),
            open: std::mem::transmute::<_, PfnWtOpenA>(open),
            close: std::mem::transmute::<_, PfnWtClose>(close),
            get: std::mem::transmute::<_, PfnWtGetA>(get),
            set: std::mem::transmute::<_, PfnWtSetA>(set),
        })
    })
    .as_ref()
}

/// Retrieve the driver-provided default LOGCONTEXT template.
/// Retrieve the driver-provided default LOGCONTEXT template (best‑effort).
///
/// Some drivers may return a partially filled structure; we still proceed since most fields
/// of interest (tablet input extents) are typically present.
pub fn wt_info_defcontext() -> Result<LOGCONTEXTA> {
    let f = load_wintab().ok_or_else(|| anyhow!("wintab32.dll not available"))?;
    let mut ctx = LOGCONTEXTA::default();
    let sz = unsafe { (f.info)(WTI_DEFCONTEXT, 0, &mut ctx as *mut _ as *mut _) };
    if sz == 0 {
        return Err(anyhow!("WTInfoA WTI_DEFCONTEXT failed (size=0)"));
    }
    // Some drivers return partial; we proceed anyway.
    Ok(ctx)
}

/// Open a WinTab context for the given window handle.
/// Open a WinTab context for the given window handle using the supplied context template.
pub fn wt_open(hwnd: HWND, ctx: &LOGCONTEXTA) -> Result<HCTX> {
    let f = load_wintab().ok_or_else(|| anyhow!("wintab32.dll not available"))?;
    let h = unsafe { (f.open)(hwnd, ctx as *const _, 1) };
    if h == 0 {
        return Err(anyhow!("WTOpenA returned NULL"));
    }
    Ok(h)
}

#[allow(dead_code)]
/// Query current LOGCONTEXT state for an open context.
pub fn wt_get(hctx: HCTX) -> Result<LOGCONTEXTA> {
    let mut ctx = LOGCONTEXTA::default();
    let f = load_wintab().ok_or_else(|| anyhow!("wintab32.dll not available"))?;
    if unsafe { (f.get)(hctx, &mut ctx as *mut _) } == 0 {
        return Err(anyhow!("WTGetA failed"));
    }
    Ok(ctx)
}

/// Apply a LOGCONTEXT to an existing context.
pub fn wt_set(hctx: HCTX, ctx: &LOGCONTEXTA) -> Result<()> {
    let f = load_wintab().ok_or_else(|| anyhow!("wintab32.dll not available"))?;
    if unsafe { (f.set)(hctx, ctx as *const _) } == 0 {
        return Err(anyhow!("WTSetA failed"));
    }
    Ok(())
}

#[allow(dead_code)]
/// Close a context (best-effort; ignores errors and missing DLL).
pub fn wt_close(hctx: HCTX) {
    if let Some(f) = load_wintab() {
        unsafe {
            let _ = (f.close)(hctx);
        }
    }
}
