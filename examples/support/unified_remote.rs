// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT
use std::{
  ffi::c_void,
  ptr,
  sync::{Arc, Mutex},
};
use windows::{
  core::{w, Result, PCWSTR},
  Win32::{
    Foundation::*,
    Networking::WinHttp::*,
    UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON},
  },
};
use wry::WebView2VirtualFile;

pub const NAME: &str = "SynckHub.Desktop_1.8.0_aarch64.app.tar.gz";
pub const SIZE: u64 = 62_577_581;
const BLOCK: u64 = 1024 * 1024;
struct Handle(*mut c_void);
impl Handle {
  fn new(p: *mut c_void) -> Result<Self> {
    if p.is_null() {
      Err(windows::core::Error::from_thread())
    } else {
      Ok(Self(p))
    }
  }
}
impl Drop for Handle {
  fn drop(&mut self) {
    unsafe {
      let _ = WinHttpCloseHandle(self.0);
    }
  }
}
struct Cache {
  offset: u64,
  bytes: Vec<u8>,
  session: u64,
}
impl Drop for Cache {
  fn drop(&mut self) {
    println!("PROVIDER_RELEASED session={}", self.session);
  }
}

// Immutable public fixture metadata; there is no request during construction.
pub fn file(session: u64) -> WebView2VirtualFile {
  let cache = Mutex::new(Cache {
    offset: u64::MAX,
    bytes: Vec::new(),
    session,
  });
  WebView2VirtualFile {
    name: NAME.into(),
    size: SIZE,
    read_at: Arc::new(move |offset, out| {
      println!(
        "READ session={session} offset={offset} count={} left_down={}",
        out.len(),
        unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 }
      );
      let mut cache = cache.lock().map_err(|_| E_FAIL)?;
      let mut done = 0;
      while done < out.len() {
        let position = offset + done as u64;
        let block = (position / BLOCK) * BLOCK;
        if cache.offset != block {
          cache.offset = u64::MAX;
          cache.bytes = fetch(session, block, BLOCK.min(SIZE - block) as u32)?;
          cache.offset = block;
        }
        let within = (position - block) as usize;
        let count = (out.len() - done).min(cache.bytes.len() - within);
        out[done..done + count].copy_from_slice(&cache.bytes[within..within + count]);
        done += count;
      }
      Ok(done)
    }),
  }
}

fn fetch(session: u64, offset: u64, count: u32) -> Result<Vec<u8>> {
  unsafe {
    println!(
      "NETWORK_REQUEST session={session} offset={offset} count={count} left_down={}",
      GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0
    );
    let client = Handle::new(WinHttpOpen(
      w!("WryUnifiedDrag/1"),
      WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
      PCWSTR::null(),
      PCWSTR::null(),
      0,
    ))?;
    WinHttpSetTimeouts(client.0, 15000, 15000, 30000, 30000)?;
    let connection = Handle::new(WinHttpConnect(client.0, w!("github.com"), 443, 0))?;
    let request=Handle::new(WinHttpOpenRequest(connection.0,w!("GET"),w!("/synergyos/synckhub-desktop-releases/releases/download/synckhub-desktop-v1.8.0/SynckHub.Desktop_1.8.0_aarch64.app.tar.gz"),PCWSTR::null(),PCWSTR::null(),ptr::null(),WINHTTP_FLAG_SECURE))?;
    WinHttpSetOption(
      Some(request.0),
      WINHTTP_OPTION_DISABLE_FEATURE,
      Some(&(WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION).to_ne_bytes()),
    )?;
    WinHttpSetOption(
      Some(request.0),
      WINHTTP_OPTION_REDIRECT_POLICY,
      Some(&WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP.to_ne_bytes()),
    )?;
    WinHttpSetOption(
      Some(request.0),
      WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS,
      Some(&5u32.to_ne_bytes()),
    )?;
    let headers: Vec<u16> = format!(
      "Range: bytes={offset}-{}\r\nAccept-Encoding: identity\r\nCache-Control: no-cache\r\n",
      offset + u64::from(count) - 1
    )
    .encode_utf16()
    .collect();
    WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0)?;
    WinHttpReceiveResponse(request.0, ptr::null_mut())?;
    let mut status = 0u32;
    let mut length = 0u32;
    let mut size = 4;
    WinHttpQueryHeaders(
      request.0,
      WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
      PCWSTR::null(),
      Some((&mut status as *mut u32).cast()),
      &mut size,
      ptr::null_mut(),
    )?;
    size = 4;
    WinHttpQueryHeaders(
      request.0,
      WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
      PCWSTR::null(),
      Some((&mut length as *mut u32).cast()),
      &mut size,
      ptr::null_mut(),
    )?;
    let mut range = [0u16; 128];
    size = 256;
    WinHttpQueryHeaders(
      request.0,
      WINHTTP_QUERY_CONTENT_RANGE,
      PCWSTR::null(),
      Some(range.as_mut_ptr().cast()),
      &mut size,
      ptr::null_mut(),
    )?;
    let range =
      String::from_utf16_lossy(&range[..range.iter().position(|c| *c == 0).unwrap_or(range.len())]);
    let expected = format!("bytes {offset}-{}/{SIZE}", offset + u64::from(count) - 1);
    if status != 206 || length != count || range != expected {
      return Err(windows::core::Error::new(
        E_FAIL,
        format!("Unexpected range response: {status} {length} {range}"),
      ));
    }
    let mut bytes = vec![0; count as usize];
    let mut done = 0;
    while done < bytes.len() {
      let mut got = 0;
      WinHttpReadData(
        request.0,
        bytes[done..].as_mut_ptr().cast(),
        (bytes.len() - done) as u32,
        &mut got,
      )?;
      if got == 0 {
        return Err(STG_E_READFAULT.into());
      }
      done += got as usize;
    }
    let mut extra = 0u8;
    let mut got = 0;
    WinHttpReadData(request.0, (&mut extra as *mut u8).cast(), 1, &mut got)?;
    if got != 0 {
      return Err(STG_E_READFAULT.into());
    }
    println!("NETWORK_COMPLETE session={session} offset={offset} count={count} status={status}");
    Ok(bytes)
  }
}
