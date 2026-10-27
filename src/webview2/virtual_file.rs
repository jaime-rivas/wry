// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Delayed streams attached to the browser's original drag object. The browser
//! retains ownership of its source loop and therefore of DOM drag completion.

use std::{
  ffi::c_void,
  mem::{size_of, ManuallyDrop},
  ptr,
  sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
  },
};
use windows::{
  core::{implement, Ref, Result, HRESULT, PWSTR},
  Win32::{
    Foundation::*,
    System::{Com::*, DataExchange::RegisterClipboardFormatW, Memory::*, Ole::*},
    UI::Shell::*,
  },
};

/// A file offered by a genuine HTML drag, with content read only on demand.
///
/// The name is a single Windows file name, not a path. The size must be known
/// before dragging. `read_at` must fill the requested buffer at the absolute offset,
/// without changing the source. It can be called repeatedly, out of order, or
/// after the source drag has ended. It must not panic. Keep credentials/source
/// ownership in the closure, not in the webview. No local file is required.
/// Short reads before the declared end of the file are reported as read errors.
///
/// Calls are synchronous COM stream reads. Slow network reads may block the UI
/// thread; this API does not promise asynchronous Shell transfers.
#[derive(Clone)]
pub struct WebView2VirtualFile {
  pub name: String,
  pub size: u64,
  pub read_at: Arc<dyn Fn(u64, &mut [u8]) -> Result<usize> + Send + Sync>,
}

pub(super) struct Session {
  files: Vec<WebView2VirtualFile>,
  ready: AtomicBool,
}

#[derive(Clone, Copy)]
pub(super) struct Formats {
  pub descriptor: u16,
  pub contents: u16,
}
impl Formats {
  pub fn register() -> Result<Self> {
    let descriptor = unsafe { RegisterClipboardFormatW(CFSTR_FILEDESCRIPTORW) } as u16;
    let contents = unsafe { RegisterClipboardFormatW(CFSTR_FILECONTENTS) } as u16;
    if descriptor == 0 || contents == 0 {
      return Err(windows::core::Error::from_thread());
    }
    Ok(Self {
      descriptor,
      contents,
    })
  }
  pub fn contains(&self, format: u16) -> bool {
    format == self.descriptor || format == self.contents
  }
}

pub(super) fn format(cf: u16, tymed: TYMED, index: i32) -> FORMATETC {
  FORMATETC {
    cfFormat: cf,
    ptd: ptr::null_mut(),
    dwAspect: DVASPECT_CONTENT.0,
    lindex: index,
    tymed: tymed.0 as u32,
  }
}

fn valid_name(name: &str) -> bool {
  let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
  !name.is_empty()
    && name.encode_utf16().count() < 260
    && !name.ends_with(['.', ' '])
    && !name.chars().any(|c| c < ' ' || "<>:\"/\\|?*".contains(c))
    && !matches!(
      stem.as_str(),
      "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    )
    && !(stem.starts_with("COM") || stem.starts_with("LPT"))
      .then(|| &stem[3..])
      .is_some_and(|s| {
        matches!(
          s,
          "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
      })
}

/// Install only metadata and stream objects. Streams own the session, which
/// never references the browser IDataObject (no COM reference cycle).
pub(super) fn install(
  data: &IDataObject,
  files: Vec<WebView2VirtualFile>,
  formats: Formats,
) -> Result<Weak<Session>> {
  if files.is_empty()
    || files.len() > i32::MAX as usize
    || files.iter().any(|f| !valid_name(&f.name))
  {
    return Err(E_INVALIDARG.into());
  }
  let mut names = std::collections::HashSet::new();
  if files.iter().any(|f| !names.insert(f.name.to_uppercase())) {
    return Err(E_INVALIDARG.into());
  }
  // Never replace formats supplied by the browser or another source provider.
  let enumerator = unsafe { data.EnumFormatEtc(DATADIR_GET.0 as u32)? };
  loop {
    let mut entry = FORMATETC::default();
    let hr = unsafe { enumerator.Next(std::slice::from_mut(&mut entry), None) };
    if hr == S_FALSE {
      break;
    }
    hr.ok()?;
    let collision = formats.contains(entry.cfFormat);
    unsafe {
      CoTaskMemFree(Some(entry.ptd.cast()));
    }
    if collision {
      return Err(DV_E_FORMATETC.into());
    }
  }
  let session = Arc::new(Session {
    files,
    ready: AtomicBool::new(false),
  });
  let length = size_of::<FILEGROUPDESCRIPTORW>()
    .checked_add(
      (session.files.len() - 1)
        .checked_mul(size_of::<FILEDESCRIPTORW>())
        .ok_or(E_OUTOFMEMORY)?,
    )
    .ok_or(E_OUTOFMEMORY)?;
  unsafe {
    let memory = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, length)?;
    let group = GlobalLock(memory).cast::<FILEGROUPDESCRIPTORW>();
    if group.is_null() {
      let _ = GlobalFree(Some(memory));
      return Err(E_OUTOFMEMORY.into());
    }
    (*group).cItems = session.files.len() as u32;
    let descriptors = ptr::addr_of_mut!((*group).fgd).cast::<FILEDESCRIPTORW>();
    for (index, file) in session.files.iter().enumerate() {
      let descriptor = &mut *descriptors.add(index);
      descriptor.dwFlags = (FD_ATTRIBUTES.0 | FD_FILESIZE.0 | FD_UNICODE.0) as u32;
      descriptor.dwFileAttributes = 0x80; // FILE_ATTRIBUTE_NORMAL
      descriptor.nFileSizeHigh = (file.size >> 32) as u32;
      descriptor.nFileSizeLow = file.size as u32;
      let mut name = [0u16; 260];
      for (out, value) in name.iter_mut().zip(file.name.encode_utf16()) {
        *out = value;
      }
      descriptor.cFileName = name;
    }
    let _ = GlobalUnlock(memory);
    let mut medium = STGMEDIUM {
      tymed: TYMED_HGLOBAL.0 as u32,
      u: STGMEDIUM_0 { hGlobal: memory },
      pUnkForRelease: ManuallyDrop::new(None),
    };
    if let Err(error) = data.SetData(
      &format(formats.descriptor, TYMED_HGLOBAL, -1),
      &medium,
      true,
    ) {
      ReleaseStgMedium(&mut medium);
      return Err(error);
    }
    for index in 0..session.files.len() {
      let stream: IStream = LazyStream {
        session: session.clone(),
        index,
        position: Mutex::new(0),
      }
      .into();
      let mut medium = STGMEDIUM {
        tymed: TYMED_ISTREAM.0 as u32,
        u: STGMEDIUM_0 {
          pstm: ManuallyDrop::new(Some(stream)),
        },
        pUnkForRelease: ManuallyDrop::new(None),
      };
      if let Err(error) = data.SetData(
        &format(formats.contents, TYMED_ISTREAM, index as i32),
        &medium,
        true,
      ) {
        ReleaseStgMedium(&mut medium);
        return Err(error);
      }
    }
  }
  session.ready.store(true, Ordering::Release);
  Ok(Arc::downgrade(&session))
}

// Match the classic COM stream contract used by the standalone probe. Do not
// opt into windows-rs's automatic agile/WinRT marshaler for an OLE drag stream.
#[implement(IStream, Agile = false)]
struct LazyStream {
  session: Arc<Session>,
  index: usize,
  position: Mutex<u64>,
}

impl ISequentialStream_Impl for LazyStream_Impl {
  fn Read(&self, buffer: *mut c_void, count: u32, read: *mut u32) -> HRESULT {
    unsafe {
      if !read.is_null() {
        *read = 0;
      }
    }
    if count == 0 {
      return S_OK;
    }
    if buffer.is_null() {
      return STG_E_INVALIDPOINTER;
    }
    if !self.session.ready.load(Ordering::Acquire) {
      return E_UNEXPECTED;
    }
    let Ok(mut position) = self.position.lock() else {
      return E_FAIL;
    };
    let file = &self.session.files[self.index];
    let take = u64::from(count).min(file.size.saturating_sub(*position)) as usize;
    if take == 0 {
      return S_FALSE;
    }
    let bytes = unsafe { std::slice::from_raw_parts_mut(buffer.cast::<u8>(), take) };
    match (file.read_at)(*position, bytes) {
      Ok(n) if n == take => {
        *position += n as u64;
        unsafe {
          if !read.is_null() {
            *read = n as u32;
          }
        }
        if n == count as usize {
          S_OK
        } else {
          S_FALSE
        }
      }
      Ok(n) if n < take => STG_E_READFAULT,
      Ok(_) => E_UNEXPECTED,
      Err(error) => error.code(),
    }
  }
  fn Write(&self, _: *const c_void, _: u32, written: *mut u32) -> HRESULT {
    unsafe {
      if !written.is_null() {
        *written = 0;
      }
    }
    STG_E_ACCESSDENIED
  }
}

impl IStream_Impl for LazyStream_Impl {
  fn Seek(&self, delta: i64, origin: STREAM_SEEK, out: *mut u64) -> Result<()> {
    let mut position = self.position.lock().map_err(|_| E_FAIL)?;
    let base = match origin {
      STREAM_SEEK_SET => 0,
      STREAM_SEEK_CUR => *position,
      STREAM_SEEK_END => self.session.files[self.index].size,
      _ => return Err(STG_E_INVALIDFUNCTION.into()),
    };
    *position = if delta < 0 {
      base.checked_sub(delta.unsigned_abs())
    } else {
      base.checked_add(delta as u64)
    }
    .ok_or(STG_E_INVALIDFUNCTION)?;
    unsafe {
      if !out.is_null() {
        *out = *position;
      }
    }
    Ok(())
  }
  fn SetSize(&self, _: u64) -> Result<()> {
    Err(STG_E_ACCESSDENIED.into())
  }
  fn CopyTo(
    &self,
    target: Ref<IStream>,
    count: u64,
    read: *mut u64,
    written: *mut u64,
  ) -> Result<()> {
    unsafe {
      if !read.is_null() {
        *read = 0;
      }
      if !written.is_null() {
        *written = 0;
      }
    }
    let target = target.as_ref().ok_or(STG_E_INVALIDPOINTER)?;
    let mut copied = 0;
    let mut bytes = [0u8; 65536];
    while copied < count {
      let amount = (count - copied).min(bytes.len() as u64) as u32;
      let mut got = 0;
      let mut sent = 0;
      let hr = self.Read(bytes.as_mut_ptr().cast(), amount, &mut got);
      unsafe {
        if !read.is_null() {
          *read += got as u64;
        }
      }
      hr.ok()?;
      if got != 0 {
        let hr = unsafe { target.Write(bytes.as_ptr().cast(), got, Some(&mut sent)) };
        unsafe {
          if !written.is_null() {
            *written += sent as u64;
          }
        }
        hr.ok()?;
        if sent != got {
          return Err(STG_E_MEDIUMFULL.into());
        }
      }
      copied += got as u64;
      if got < amount {
        break;
      }
    }
    Ok(())
  }
  fn Commit(&self, _: &STGC) -> Result<()> {
    Ok(())
  }
  fn Revert(&self) -> Result<()> {
    Err(STG_E_REVERTED.into())
  }
  fn LockRegion(&self, _: u64, _: u64, _: &LOCKTYPE) -> Result<()> {
    Err(STG_E_INVALIDFUNCTION.into())
  }
  fn UnlockRegion(&self, _: u64, _: u64, _: u32) -> Result<()> {
    Err(STG_E_INVALIDFUNCTION.into())
  }
  fn Stat(&self, out: *mut STATSTG, flags: &STATFLAG) -> Result<()> {
    if out.is_null() {
      return Err(STG_E_INVALIDPOINTER.into());
    }
    let file = &self.session.files[self.index];
    let mut stat = STATSTG {
      r#type: STGTY_STREAM.0 as u32,
      cbSize: file.size,
      grfMode: STGM_READ,
      ..Default::default()
    };
    if flags.0 & STATFLAG_NONAME.0 == 0 {
      let name: Vec<u16> = file.name.encode_utf16().chain(Some(0)).collect();
      let raw = unsafe { CoTaskMemAlloc(name.len() * 2) }.cast::<u16>();
      if raw.is_null() {
        return Err(E_OUTOFMEMORY.into());
      }
      unsafe {
        ptr::copy_nonoverlapping(name.as_ptr(), raw, name.len());
      }
      stat.pwcsName = PWSTR(raw);
    }
    unsafe {
      out.write(stat);
    }
    Ok(())
  }
  fn Clone(&self) -> Result<IStream> {
    Ok(
      LazyStream {
        session: self.session.clone(),
        index: self.index,
        position: Mutex::new(*self.position.lock().map_err(|_| E_FAIL)?),
      }
      .into(),
    )
  }
}

#[cfg(test)]
#[path = "virtual_file_tests.rs"]
mod tests;
