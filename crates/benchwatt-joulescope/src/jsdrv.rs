//! The few calls of the vendor driver that the rest of the crate needs, with
//! the pointer handling kept in one place.
use benchwatt_device::{Error, ErrorKind, Result};
use jsdrv_sys as sys;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    ptr,
    sync::OnceLock,
};

pub use sys::jsdrv_statistics_s as Statistics;

const TIMEOUT_MS: u32 = sys::JSDRV_TIMEOUT_MS_DEFAULT;

/// What a subscription delivers.
pub enum Message<'a> {
    Number(f64),
    Text(&'a str),
    Bytes(&'a [u8]),
    Null,
}
/// Called on the driver's own thread; must not block for long.
pub type Callback = fn(user: *const c_void, topic: &str, message: Message);

pub struct Driver(*mut sys::jsdrv_context_s);
// The vendor documents its context as usable from any thread.
unsafe impl Send for Driver {}
unsafe impl Sync for Driver {}

/// The one driver instance of this process. It watches the USB bus from its
/// own threads for as long as the process lives.
pub fn driver() -> Result<&'static Driver> {
    static DRIVER: OnceLock<std::result::Result<Driver, i32>> = OnceLock::new();
    DRIVER
        .get_or_init(|| {
            let mut ctx = ptr::null_mut();
            match unsafe {
                sys::jsdrv_initialize(&mut ctx, ptr::null(), sys::JSDRV_TIMEOUT_MS_INIT)
            } {
                0 => Ok(Driver(ctx)),
                rc => Err(rc),
            }
        })
        .as_ref()
        .map_err(|&rc| error("starting the Joulescope driver", rc))
}

fn error(what: &str, rc: i32) -> Error {
    let name = unsafe { CStr::from_ptr(sys::jsdrv_error_code_description(rc)) }.to_string_lossy();
    let kind = match rc as u32 {
        sys::JSDRV_ERROR_TIMED_OUT => ErrorKind::Timeout,
        sys::JSDRV_ERROR_PARAMETER_INVALID => ErrorKind::Invalid,
        sys::JSDRV_ERROR_CLOSED | sys::JSDRV_ERROR_NOT_FOUND => ErrorKind::Closed,
        _ => ErrorKind::Io,
    };
    Error::new(kind, format!("{what}: {name}"))
}
fn check(what: &str, rc: i32) -> Result<()> {
    match rc {
        0 => Ok(()),
        rc => Err(error(what, rc)),
    }
}
fn cstr(s: &str) -> CString {
    CString::new(s).expect("topic without NUL")
}
fn value(type_: u32, value: sys::jsdrv_union_inner_u) -> sys::jsdrv_union_s {
    sys::jsdrv_union_s {
        type_: type_ as u8,
        flags: 0,
        op: 0,
        app: 0,
        size: 0,
        value,
    }
}

struct Subscriber {
    callback: Callback,
    user: *const c_void,
}
unsafe extern "C" fn trampoline(
    user: *mut c_void,
    topic: *const c_char,
    v: *const sys::jsdrv_union_s,
) {
    let (s, v) = unsafe { (&*(user as *const Subscriber), &*v) };
    let Ok(topic) = unsafe { CStr::from_ptr(topic) }.to_str() else {
        return;
    };
    let message = unsafe {
        match u32::from(v.type_) {
            sys::JSDRV_UNION_NULL => Message::Null,
            sys::JSDRV_UNION_STR | sys::JSDRV_UNION_JSON => {
                Message::Text(CStr::from_ptr(v.value.str_).to_str().unwrap_or_default())
            }
            sys::JSDRV_UNION_BIN => {
                Message::Bytes(std::slice::from_raw_parts(v.value.bin, v.size as usize))
            }
            sys::JSDRV_UNION_F32 => Message::Number(v.value.f32_.into()),
            sys::JSDRV_UNION_F64 => Message::Number(v.value.f64_),
            sys::JSDRV_UNION_I8 => Message::Number(v.value.i8_.into()),
            sys::JSDRV_UNION_I16 => Message::Number(v.value.i16_.into()),
            sys::JSDRV_UNION_I32 => Message::Number(v.value.i32_.into()),
            sys::JSDRV_UNION_I64 => Message::Number(v.value.i64_ as f64),
            sys::JSDRV_UNION_U8 => Message::Number(v.value.u8_.into()),
            sys::JSDRV_UNION_U16 => Message::Number(v.value.u16_.into()),
            sys::JSDRV_UNION_U32 => Message::Number(v.value.u32_.into()),
            sys::JSDRV_UNION_U64 => Message::Number(v.value.u64_ as f64),
            _ => return,
        }
    };
    (s.callback)(s.user, topic, message);
}
/// Ends when dropped; by then the callback is guaranteed not to run again.
pub struct Subscription {
    driver: &'static Driver,
    topic: CString,
    subscriber: Box<Subscriber>,
}
unsafe impl Send for Subscription {}
unsafe impl Sync for Subscription {}
impl Drop for Subscription {
    fn drop(&mut self) {
        unsafe {
            sys::jsdrv_unsubscribe(
                self.driver.0,
                self.topic.as_ptr(),
                Some(trampoline),
                &raw const *self.subscriber as *mut c_void,
                TIMEOUT_MS,
            );
        }
    }
}

impl Driver {
    /// Device names such as `u/js220/005445`.
    pub fn devices(&self) -> Vec<String> {
        let mut buf = [0u8; 1024];
        let mut list = value(
            sys::JSDRV_UNION_STR,
            sys::jsdrv_union_inner_u {
                str_: buf.as_mut_ptr().cast(),
            },
        );
        list.size = buf.len() as u32;
        if unsafe { sys::jsdrv_query(self.0, c"@/list".as_ptr(), &mut list, TIMEOUT_MS) } != 0 {
            return Vec::new();
        }
        CStr::from_bytes_until_nul(&buf)
            .ok()
            .and_then(|s| s.to_str().ok())
            .unwrap_or_default()
            .split(',')
            .filter(|d| !d.is_empty())
            .map(str::to_owned)
            .collect()
    }
    /// Opens the device as it is, without putting its settings back to defaults.
    pub fn open(&self, device: &str) -> Result<()> {
        let rc = unsafe {
            sys::jsdrv_open(
                self.0,
                cstr(device).as_ptr(),
                sys::JSDRV_DEVICE_OPEN_MODE_RESUME as i32,
                sys::JSDRV_TIMEOUT_MS_OPEN,
            )
        };
        check(&format!("opening {device}"), rc)
    }
    pub fn close(&self, device: &str) {
        unsafe { sys::jsdrv_close(self.0, cstr(device).as_ptr(), TIMEOUT_MS) };
    }
    /// A setting is published as retained, so the instrument keeps it and can
    /// be asked for it later. A command (`!name`) is not.
    pub fn publish(&self, topic: &str, v: u32) -> Result<()> {
        let mut v = value(
            sys::JSDRV_UNION_U32,
            sys::jsdrv_union_inner_u { u64_: v.into() },
        );
        if !topic
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with('!'))
        {
            v.flags = sys::JSDRV_UNION_FLAG_RETAIN as u8;
        }
        let rc = unsafe { sys::jsdrv_publish(self.0, cstr(topic).as_ptr(), &v, TIMEOUT_MS) };
        check(&format!("setting {topic}"), rc)
    }
    /// The last value of a topic that holds a number, if it has one.
    pub fn query(&self, topic: &str) -> Result<Option<u32>> {
        let mut v = value(sys::JSDRV_UNION_NULL, sys::jsdrv_union_inner_u { u64_: 0 });
        let rc = unsafe { sys::jsdrv_query(self.0, cstr(topic).as_ptr(), &mut v, TIMEOUT_MS) };
        check(&format!("reading {topic}"), rc)?;
        Ok((u32::from(v.type_) != sys::JSDRV_UNION_NULL).then_some(unsafe { v.value.u32_ }))
    }
    /// `user` is handed to every call of `callback` and must stay valid for
    /// as long as the subscription lives.
    pub fn subscribe(
        &'static self,
        topic: &str,
        callback: Callback,
        user: *const c_void,
    ) -> Result<Subscription> {
        let subscription = Subscription {
            driver: self,
            topic: cstr(topic),
            subscriber: Box::new(Subscriber { callback, user }),
        };
        let rc = unsafe {
            sys::jsdrv_subscribe(
                self.0,
                subscription.topic.as_ptr(),
                sys::JSDRV_SFLAG_PUB as u8,
                Some(trampoline),
                &raw const *subscription.subscriber as *mut c_void,
                TIMEOUT_MS,
            )
        };
        match rc {
            0 => Ok(subscription),
            rc => {
                // Nothing to unsubscribe.
                std::mem::forget(subscription);
                Err(error(&format!("subscribing to {topic}"), rc))
            }
        }
    }
}
