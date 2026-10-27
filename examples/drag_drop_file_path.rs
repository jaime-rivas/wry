// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

#[cfg(target_os = "windows")]
fn main() -> wry::Result<()> {
  use std::{cell::Cell, fs::File, io::Read};
  use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
  };
  use wry::{WebView2FilePaths, WebViewBuilder};

  let event_loop = EventLoop::new();
  let window = WindowBuilder::new()
    .with_title("Wry — HTML drops with native file access")
    .build(&event_loop)
    .unwrap();
  let _webview = WebViewBuilder::new()
    .with_html(include_str!("drag-drop-file-path.html"))
    .with_navigation_handler({
      let initial = Cell::new(true);
      move |url| {
        initial.replace(false) && (url == "about:blank" || url.starts_with("data:text/html;"))
      }
    })
    .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
    // No native drag-drop handler: let the browser own its genuine DOM events.
    .with_ipc_handler(|request| {
      if request.uri() != "about:blank" {
        return;
      }
      println!("DOM {}", request.body());
      match request.extensions().get::<WebView2FilePaths>() {
        None => println!("FILES none"),
        Some(WebView2FilePaths(Err(error))) => println!("FILES error {error:?}"),
        Some(WebView2FilePaths(Ok(paths))) => {
          println!("FILES count={}", paths.len());
          for (index, path) in paths.iter().enumerate() {
            let observation = (|| -> std::io::Result<_> {
              let metadata = std::fs::metadata(path)?;
              let mut sample = [0; 32];
              let read = if metadata.is_file() {
                File::open(path)?.read(&mut sample)?
              } else {
                0
              };
              Ok((metadata.is_dir(), read))
            })();
            // Only use test fixtures: this example opens the files read-only.
            println!("FILE index={index} path={path:?} access={observation:?}");
          }
        }
      }
    })
    .build(&window)?;

  event_loop.run(move |event, _, control_flow| {
    *control_flow = ControlFlow::Wait;
    if let Event::WindowEvent {
      event: WindowEvent::CloseRequested,
      ..
    } = event
    {
      *control_flow = ControlFlow::Exit;
    }
    // Keep the webview alive for the window's lifetime.
    let _ = &_webview;
  });
}

#[cfg(not(target_os = "windows"))]
fn main() {
  eprintln!("This example currently requires Windows and WebView2.");
}
