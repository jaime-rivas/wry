// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};

use webview2_com::{CoTaskMemPWSTR, Microsoft::Web::WebView2::Win32::*};
use windows::{
  Win32::Foundation::{E_INVALIDARG, E_NOINTERFACE},
  core::{Interface, PWSTR},
};

/// Native paths of DOM files attached to a Windows IPC message.
///
/// Send a string and an array of DOM `File` objects with
/// `window.ipc.postMessage(message, files)`, then retrieve this value from
/// [`http::Request::extensions`] in [`crate::WebViewBuilder::with_ipc_handler`].
/// Ordinary messages without additional objects have no such extension.
///
/// The result contains one path per attached file, in the same order, or the
/// error encountered while reading the objects. It never contains a partial
/// list. Non-file objects and files without native backing (such as JavaScript
/// `new File(...)`) produce an error. A missing or empty native path is reported
/// as `E_INVALIDARG`.
///
/// These paths come from the objects attached to this message, not from its
/// string body. They do not prove a trusted drop or grant filesystem permission.
/// Validate the request URL and apply application permissions before opening a
/// path. Paths can change after the callback; retain an opened handle or capture
/// content if the application needs durable access. Wry does not open the files.
#[derive(Clone, Debug)]
pub struct WebView2FilePaths(pub windows_core::Result<Vec<PathBuf>>);

pub(super) fn file_paths(
  args: &ICoreWebView2WebMessageReceivedEventArgs,
) -> Option<WebView2FilePaths> {
  let args = match args.cast::<ICoreWebView2WebMessageReceivedEventArgs2>() {
    Ok(args) => args,
    Err(error) if error.code() == E_NOINTERFACE => return None,
    Err(error) => return Some(WebView2FilePaths(Err(error))),
  };
  // The property is null for ordinary messages. The generated non-null COM
  // return type cannot represent that documented success case, so read the
  // nullable out parameter directly rather than treating E_POINTER as success.
  let mut raw = std::ptr::null_mut();
  let result = unsafe { (args.vtable().AdditionalObjects)(args.as_raw(), &mut raw).ok() };
  if let Err(error) = result {
    return Some(WebView2FilePaths(Err(error)));
  }
  if raw.is_null() {
    return None;
  }
  // The successful COM getter transferred one reference to us.
  let objects = unsafe { ICoreWebView2ObjectCollectionView::from_raw(raw) };
  Some(WebView2FilePaths(paths_from_objects(&objects)))
}

fn paths_from_objects(
  objects: &ICoreWebView2ObjectCollectionView,
) -> windows_core::Result<Vec<PathBuf>> {
  let mut count = 0;
  unsafe { objects.Count(&mut count)? };
  (0..count)
    .map(|index| {
      let file = unsafe { objects.GetValueAtIndex(index)? }.cast::<ICoreWebView2File>()?;
      let mut path = PWSTR::null();
      let result = unsafe { file.Path(&mut path) };
      // Free even a partially returned allocation when the getter fails.
      let _allocation = CoTaskMemPWSTR::from(path);
      result?;
      if path.is_null() || unsafe { path.as_wide() }.is_empty() {
        return Err(E_INVALIDARG.into());
      }
      // Filesystem paths must retain Windows UTF-16, including unpaired surrogates.
      Ok(PathBuf::from(OsString::from_wide(unsafe {
        path.as_wide()
      })))
    })
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::os::windows::ffi::OsStrExt;
  use windows::{
    Win32::{Foundation::E_ACCESSDENIED, System::Com::CoTaskMemAlloc},
    core::{IUnknown, implement},
  };

  #[implement(ICoreWebView2File)]
  struct File {
    path: Vec<u16>,
    fail: bool,
  }

  impl ICoreWebView2File_Impl for File_Impl {
    fn Path(&self, out: *mut PWSTR) -> windows_core::Result<()> {
      let buffer = unsafe { CoTaskMemAlloc((self.path.len() + 1) * 2) } as *mut u16;
      assert!(!buffer.is_null());
      unsafe {
        std::ptr::copy_nonoverlapping(self.path.as_ptr(), buffer, self.path.len());
        buffer.add(self.path.len()).write(0);
        out.write(PWSTR(buffer));
      }
      if self.fail {
        Err(E_ACCESSDENIED.into())
      } else {
        Ok(())
      }
    }
  }

  #[implement(ICoreWebView2ObjectCollectionView)]
  struct Objects(Vec<IUnknown>);

  impl ICoreWebView2ObjectCollectionView_Impl for Objects_Impl {
    fn Count(&self, out: *mut u32) -> windows_core::Result<()> {
      unsafe { out.write(self.0.len() as u32) };
      Ok(())
    }

    fn GetValueAtIndex(&self, index: u32) -> windows_core::Result<IUnknown> {
      self
        .0
        .get(index as usize)
        .cloned()
        .ok_or_else(|| E_INVALIDARG.into())
    }
  }

  fn file(path: &[u16], fail: bool) -> IUnknown {
    let file: ICoreWebView2File = File {
      path: path.to_vec(),
      fail,
    }
    .into();
    file.cast().unwrap()
  }

  #[test]
  fn file_paths_preserve_order_duplicates_and_windows_encoding() {
    let first: Vec<u16> = r"C:\fixture\same.txt".encode_utf16().collect();
    let second = [b'C' as u16, b':' as u16, b'\\' as u16, 0xd800];
    let objects: ICoreWebView2ObjectCollectionView = Objects(vec![
      file(&first, false),
      file(&second, false),
      file(&first, false),
    ])
    .into();
    let paths = paths_from_objects(&objects).unwrap();
    assert_eq!(paths.len(), 3);
    assert_eq!(
      paths[0].as_os_str().encode_wide().collect::<Vec<_>>(),
      first
    );
    assert_eq!(
      paths[1].as_os_str().encode_wide().collect::<Vec<_>>(),
      second
    );
    assert_eq!(paths[2], paths[0]);
  }

  #[test]
  fn empty_file_paths_collection_is_successful() {
    let objects: ICoreWebView2ObjectCollectionView = Objects(vec![]).into();
    assert!(paths_from_objects(&objects).unwrap().is_empty());
  }

  #[test]
  fn file_paths_without_backing_path_are_errors() {
    let objects: ICoreWebView2ObjectCollectionView = Objects(vec![file(&[], false)]).into();
    assert_eq!(
      paths_from_objects(&objects).unwrap_err().code(),
      E_INVALIDARG
    );
  }

  #[test]
  fn file_paths_do_not_return_partial_success_on_getter_failure() {
    let path: Vec<u16> = r"C:\fixture.txt".encode_utf16().collect();
    let objects: ICoreWebView2ObjectCollectionView =
      Objects(vec![file(&path, false), file(&path, true)]).into();
    assert_eq!(
      paths_from_objects(&objects).unwrap_err().code(),
      E_ACCESSDENIED
    );
  }

  #[test]
  fn file_paths_reject_non_file_objects() {
    let not_a_file: ICoreWebView2ObjectCollectionView = Objects(vec![]).into();
    let objects: ICoreWebView2ObjectCollectionView =
      Objects(vec![not_a_file.cast().unwrap()]).into();
    assert_eq!(
      paths_from_objects(&objects).unwrap_err().code(),
      E_NOINTERFACE
    );
  }
}
