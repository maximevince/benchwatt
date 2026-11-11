//! Prints every topic of each connected Joulescope with its value and its
//! description, as the instrument itself reports them.
use jsdrv_sys::*;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    ptr,
};

unsafe extern "C" fn print(_: *mut c_void, topic: *const c_char, value: *const jsdrv_union_s) {
    let (topic, v) = unsafe { (CStr::from_ptr(topic).to_string_lossy(), &*value) };
    let text = unsafe {
        match v.type_ as u32 {
            JSDRV_UNION_STR | JSDRV_UNION_JSON => {
                CStr::from_ptr(v.value.str_).to_string_lossy().into_owned()
            }
            JSDRV_UNION_F32 => v.value.f32_.to_string(),
            JSDRV_UNION_F64 => v.value.f64_.to_string(),
            JSDRV_UNION_NULL => "null".into(),
            JSDRV_UNION_BIN => format!("<{} bytes>", v.size),
            JSDRV_UNION_I8 | JSDRV_UNION_I16 | JSDRV_UNION_I32 | JSDRV_UNION_I64 => {
                v.value.i64_.to_string()
            }
            _ => v.value.u64_.to_string(),
        }
    };
    println!("{topic} = {text}");
}

fn main() {
    unsafe {
        let mut ctx = ptr::null_mut();
        assert_eq!(
            jsdrv_initialize(&mut ctx, ptr::null(), JSDRV_TIMEOUT_MS_INIT),
            0
        );
        let mut buf = [0u8; 1024];
        let mut list = jsdrv_union_s {
            type_: JSDRV_UNION_STR as u8,
            flags: 0,
            op: 0,
            app: 0,
            size: buf.len() as u32,
            value: jsdrv_union_inner_u {
                str_: buf.as_mut_ptr().cast(),
            },
        };
        assert_eq!(jsdrv_query(ctx, c"@/list".as_ptr(), &mut list, 1000), 0);
        let list = CStr::from_ptr(buf.as_ptr().cast())
            .to_string_lossy()
            .into_owned();
        println!("devices: {list}");
        for device in list.split(',').filter(|d| !d.is_empty()) {
            let prefix = CString::new(device).unwrap();
            let rc = jsdrv_open(
                ctx,
                prefix.as_ptr(),
                JSDRV_DEVICE_OPEN_MODE_RESUME as i32,
                3000,
            );
            if rc != 0 {
                println!("{device}: open failed with {rc}");
                continue;
            }
            let flags = JSDRV_SFLAG_RETAIN | JSDRV_SFLAG_PUB | JSDRV_SFLAG_METADATA_RSP;
            jsdrv_subscribe(
                ctx,
                prefix.as_ptr(),
                flags as u8,
                Some(print),
                ptr::null_mut(),
                1000,
            );
            std::thread::sleep(std::time::Duration::from_millis(1500));
            jsdrv_unsubscribe(ctx, prefix.as_ptr(), Some(print), ptr::null_mut(), 1000);
            jsdrv_close(ctx, prefix.as_ptr(), 1000);
        }
        jsdrv_finalize(ctx, 1000);
    }
}
