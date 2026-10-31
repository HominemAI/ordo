// Sets or checks a file's Linux capabilities, for the image build only.
//
// The builder stage used to install Debian's `libcap2-bin` for `setcap` and
// `getcap`. CI's build containers could not reach the Debian mirrors on
// 2026-10-31, so the image writes the `security.capability` attribute
// itself. It is compiled with plain `rustc` (no crates, no network) and
// never ships in the runtime image.
//
//   setcap set   <file>   permitted: chown, setuid, setgid, setpcap
//   setcap check <file>   exit 0 if the file holds exactly those
//   setcap none  <file>   exit 0 if the file holds no capabilities
//
// The attribute is VFS_CAP_REVISION_2: a little-endian u32 of magic and
// flags, then permitted and inheritable for capabilities 0-31 and 32-63.
// No effective flag, which is what `setcap ...=p` writes.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::process::exit;

extern "C" {
    fn setxattr(path: *const c_char, name: *const c_char, value: *const c_void, size: usize, flags: c_int) -> c_int;
    fn getxattr(path: *const c_char, name: *const c_char, value: *mut c_void, size: usize) -> isize;
}

const REVISION_2: u32 = 0x0200_0000;
// CAP_CHOWN 0, CAP_SETGID 6, CAP_SETUID 7, CAP_SETPCAP 8.
const PERMITTED: u32 = (1 << 0) | (1 << 6) | (1 << 7) | (1 << 8);

fn value() -> [u8; 20] {
    let mut v = [0u8; 20];
    v[0..4].copy_from_slice(&REVISION_2.to_le_bytes());
    v[4..8].copy_from_slice(&PERMITTED.to_le_bytes());
    v
}

fn read(path: &CString, name: &CString) -> Option<Vec<u8>> {
    let mut buf = [0u8; 64];
    let n = unsafe { getxattr(path.as_ptr(), name.as_ptr(), buf.as_mut_ptr() as *mut c_void, buf.len()) };
    if n < 0 { None } else { Some(buf[..n as usize].to_vec()) }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: setcap set|check|none <file>");
        exit(2);
    }
    let path = CString::new(args[2].as_str()).unwrap();
    let name = CString::new("security.capability").unwrap();
    match args[1].as_str() {
        "set" => {
            let v = value();
            let r = unsafe { setxattr(path.as_ptr(), name.as_ptr(), v.as_ptr() as *const c_void, v.len(), 0) };
            if r != 0 {
                eprintln!("setcap: {}: {}", args[2], std::io::Error::last_os_error());
                exit(1);
            }
        }
        "check" => {
            if read(&path, &name).as_deref() != Some(&value()[..]) {
                eprintln!("setcap: {} does not hold chown, setuid, setgid, setpcap=p", args[2]);
                exit(1);
            }
        }
        "none" => {
            if read(&path, &name).is_some() {
                eprintln!("setcap: {} holds capabilities and should not", args[2]);
                exit(1);
            }
        }
        _ => {
            eprintln!("usage: setcap set|check|none <file>");
            exit(2);
        }
    }
}
