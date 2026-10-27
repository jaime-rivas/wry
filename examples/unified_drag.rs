// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

#[cfg(windows)]
#[path = "support/unified_remote.rs"]
mod remote;

#[cfg(windows)]
fn main() -> wry::Result<()> {
  use raw_window_handle::{HasWindowHandle, RawWindowHandle};
  use std::{cell::Cell, fs::File, io::Read, sync::Arc};
  use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    platform::windows::WindowBuilderExtWindows,
    window::WindowBuilder,
  };
  use windows::{
    core::Interface,
    Win32::{
      Foundation::*,
      Graphics::DirectComposition::*,
      System::{Com::*, Memory::*, Ole::*},
    },
  };
  use wry::{WebView2FilePaths, WebView2VirtualFile, WebViewBuilder, WebViewBuilderExtWindows};
  let remote_mode = std::env::args().any(|v| v == "--remote");
  let event_loop = EventLoop::new();
  let window = WindowBuilder::new()
    .with_drag_and_drop(false)
    .with_title("Wry — unified HTML and virtual-file dragging")
    .build(&event_loop)
    .unwrap();
  let hwnd = match window.window_handle()?.as_raw() {
    RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as _),
    _ => unreachable!(),
  };
  let device: IDCompositionDevice = unsafe { DCompositionCreateDevice(None)? };
  let target = unsafe { device.CreateTargetForHwnd(hwnd, true)? };
  let visual = unsafe { device.CreateVisual()? };
  unsafe {
    target.SetRoot(&visual)?;
    device.Commit()?;
  }
  let session = Cell::new(0u64);
  let webview = WebViewBuilder::new()
    .with_composition_visual_target(visual.cast()?)
    .with_virtual_file_drag_handler(move |data| {
      // Match our fixture only inside the genuine source callback. Incoming
      // messages and drops never use this text as proof of source identity.
      let f = FORMATETC {
        cfFormat: CF_UNICODETEXT.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
      };
      let matches = unsafe {
        match data.GetData(&f) {
          Ok(mut m) => {
            let mut matched = false;
            if m.tymed == TYMED_HGLOBAL.0 as u32 {
              let p = GlobalLock(m.u.hGlobal).cast::<u16>();
              if !p.is_null() {
                let expected: Vec<u16> =
                  "repository:item-1".encode_utf16().chain(Some(0)).collect();
                if GlobalSize(m.u.hGlobal) >= expected.len() * 2 {
                  matched = std::slice::from_raw_parts(p, expected.len()) == expected;
                }
                let _ = GlobalUnlock(m.u.hGlobal);
              }
            }
            ReleaseStgMedium(&mut m);
            matched
          }
          Err(_) => false,
        }
      };
      let id = session.get() + 1;
      session.set(id);
      println!("SOURCE session={id} augment={matches} remote={remote_mode}");
      if !matches {
        return Ok(vec![]);
      }
      if remote_mode {
        return Ok(vec![remote::file(id)]);
      }
      let bytes = b"Deferred content for repository:item-1\n";
      Ok(vec![WebView2VirtualFile {
        name: "wry-virtual-file.txt".into(),
        size: bytes.len() as u64,
        read_at: Arc::new(move |offset, out| {
          println!("READ session={id} offset={offset} count={}", out.len());
          out.copy_from_slice(&bytes[offset as usize..offset as usize + out.len()]);
          Ok(out.len())
        }),
      }])
    })
    .with_html(include_str!("unified-drag.html"))
    .with_navigation_handler({
      let initial = Cell::new(true);
      move |url| {
        initial.replace(false) && (url == "about:blank" || url.starts_with("data:text/html;"))
      }
    })
    .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
    .with_ipc_handler(move |request| {
      if request.uri() != "about:blank" {
        return;
      }
      println!("DOM {}", request.body());
      match request.extensions().get::<WebView2FilePaths>() {
        None => println!("FILES none"),
        Some(WebView2FilePaths(Err(e))) => println!("FILES error {e:?}"),
        Some(WebView2FilePaths(Ok(paths))) => {
          println!("FILES count={}", paths.len());
          for (i, path) in paths.iter().enumerate() {
            let observed = (|| -> std::io::Result<_> {
              let metadata = std::fs::metadata(path)?;
              let mut bytes = [0; 32];
              let read = if metadata.is_file() {
                File::open(path)?.read(&mut bytes)?
              } else {
                0
              };
              Ok((metadata.is_dir(), read))
            })();
            println!("FILE index={i} path={path:?} access={observed:?}");
          }
        }
      }
    })
    .build(&window)?;
  unsafe {
    device.Commit()?;
  }
  println!("MODE remote={remote_mode} READY_WAITING_FOR_DOM");
  event_loop.run(move |event, _, flow| {
    *flow = ControlFlow::Wait;
    if let Event::WindowEvent {
      event: WindowEvent::CloseRequested,
      ..
    } = event
    {
      *flow = ControlFlow::Exit;
    }
    let _ = (&webview, &device, &target, &visual);
  });
}

#[cfg(not(windows))]
fn main() {
  eprintln!("The unified drag example currently requires Windows.");
}
