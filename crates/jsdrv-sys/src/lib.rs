//! The C interface of Jetperch's `joulescope_driver`, built from the copy in
//! `joulescope_driver/`. The driver is Apache-2.0; it bundles libusb
//! (LGPL-2.1-or-later), miniz (MIT) and tinyprintf (BSD-3-Clause). Their
//! licence texts are in `joulescope_driver/LICENSES`.
#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
#![allow(clippy::all, unsafe_op_in_unsafe_fn)]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stream_header_is_the_size_the_driver_says() {
        assert_eq!(
            std::mem::offset_of!(jsdrv_stream_signal_s, data),
            JSDRV_STREAM_HEADER_SIZE as usize
        );
    }
}
