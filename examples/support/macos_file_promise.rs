// Copyright 2020-2026 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use block2::Block;
use objc2::{
  define_class, msg_send,
  rc::Retained,
  runtime::{NSObject, ProtocolObject},
  AllocAnyThread, DefinedClass,
};
use objc2_app_kit::{
  NSDraggingItem, NSFilePromiseProvider, NSFilePromiseProviderDelegate, NSImage,
  NSPasteboardWriting,
};
use objc2_foundation::{
  ns_string, MainThreadMarker, NSError, NSObjectProtocol, NSOperationQueue, NSString, NSURL,
};
use std::{
  fs::OpenOptions,
  io::{self, Write},
  path::Path,
  process::{Command, Stdio},
};

const REMOTE_NAME: &str = "SynckHub.Desktop_1.8.0_aarch64.app.tar.gz";
const REMOTE_URL: &str = "https://github.com/synergyos/synckhub-desktop-releases/releases/download/synckhub-desktop-v1.8.0/SynckHub.Desktop_1.8.0_aarch64.app.tar.gz";

struct Ivars {
  session: u64,
  remote: bool,
  queue: Retained<NSOperationQueue>,
}

define_class!(
  #[unsafe(super(NSObject))]
  #[ivars = Ivars]
  struct ProviderDelegate;
  unsafe impl NSObjectProtocol for ProviderDelegate {}
  unsafe impl NSFilePromiseProviderDelegate for ProviderDelegate {
    #[unsafe(method_id(filePromiseProvider:fileNameForType:))]
    fn name(&self, _provider: &NSFilePromiseProvider, _ty: &NSString) -> Retained<NSString> {
      NSString::from_str(if self.ivars().remote {
        REMOTE_NAME
      } else {
        "wry-virtual-file.txt"
      })
    }
    #[unsafe(method_id(operationQueueForFilePromiseProvider:))]
    fn queue(&self, _provider: &NSFilePromiseProvider) -> Retained<NSOperationQueue> {
      self.ivars().queue.clone()
    }
    #[unsafe(method(filePromiseProvider:writePromiseToURL:completionHandler:))]
    fn write(
      &self,
      _provider: &NSFilePromiseProvider,
      url: &NSURL,
      completion: &Block<dyn Fn(*mut NSError)>,
    ) {
      let session = self.ivars().session;
      let result = url
        .path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing destination"))
        .and_then(|path| write_file(Path::new(&path.to_string()), self.ivars().remote, session));
      match result {
        Ok(()) => completion.call((std::ptr::null_mut(),)),
        Err(error) => {
          eprintln!("WRITE_ERROR session={session} error={error}");
          let error = unsafe {
            NSError::errorWithDomain_code_userInfo(ns_string!("WryUnifiedDrag"), 1, None)
          };
          completion.call((Retained::as_ptr(&error).cast_mut(),));
        }
      }
    }
  }
);

fn write_file(path: &Path, remote: bool, session: u64) -> io::Result<()> {
  println!("CONTENT_REQUEST session={session} remote={remote} destination={path:?}");
  let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
  let result = if remote {
    // Stream directly to the requested destination, only after AppKit asks.
    // No local source, predownload, shell interpolation, or credentials.
    println!("NETWORK_REQUEST session={session}");
    Command::new("/usr/bin/curl")
      .args([
        "--disable",
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "15",
        "--max-time",
        "300",
        REMOTE_URL,
      ])
      .stdout(Stdio::from(file))
      .status()
      .and_then(|status| {
        if status.success() {
          Ok(())
        } else {
          Err(io::Error::new(
            io::ErrorKind::Other,
            format!("curl failed: {status}"),
          ))
        }
      })
  } else {
    file.write_all(b"Deferred content for repository:item-1\n")
  };
  if result.is_err() {
    // This invocation created this partial fixture with create_new.
    let _ = std::fs::remove_file(path);
  }
  result?;
  println!(
    "WRITE_COMPLETE session={session} bytes={}",
    std::fs::metadata(path)?.len()
  );
  Ok(())
}

pub fn item(
  placeholder: &NSDraggingItem,
  remote: bool,
  session: u64,
  mtm: MainThreadMarker,
) -> Retained<NSDraggingItem> {
  let queue = NSOperationQueue::new();
  queue.setMaxConcurrentOperationCount(1);
  let delegate = ProviderDelegate::alloc().set_ivars(Ivars {
    session,
    remote,
    queue,
  });
  let delegate: Retained<ProviderDelegate> = unsafe { msg_send![super(delegate), init] };
  let provider = NSFilePromiseProvider::initWithFileType_delegate(
    NSFilePromiseProvider::alloc(),
    ns_string!("public.data"),
    ProtocolObject::from_ref(&*delegate),
  );
  // AppKit's delegate property is weak. userInfo retains this delegate for the
  // provider's lifetime, including requests after the drag/window has ended.
  unsafe {
    provider.setUserInfo(Some(&delegate));
  }
  let item = NSDraggingItem::initWithPasteboardWriter(
    mtm.alloc(),
    ProtocolObject::<dyn NSPasteboardWriting>::from_ref(&*provider),
  );
  let image = NSImage::imageNamed(ns_string!("NSMultipleDocuments"));
  unsafe {
    item.setDraggingFrame_contents(
      placeholder.draggingFrame(),
      image.as_deref().map(|image| &***image),
    );
  }
  item
}
