// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use super::virtual_file::{self, ActiveSource, Formats, WebView2VirtualFile};
use std::{cell::RefCell, rc::Rc};
use webview2_com::{
  DragStartingEventHandler, Microsoft::Web::WebView2::Win32::*, NavigationStartingEventHandler,
};
use windows::{
  core::{implement, Interface, Ref, Result},
  Win32::{
    Foundation::{E_POINTER, HWND, POINT, POINTL},
    Graphics::Gdi::ScreenToClient,
    System::{Com::IDataObject, Ole::*, SystemServices::MODIFIERKEYS_FLAGS},
  },
};

pub(super) type SourceHandler = Rc<dyn Fn(&IDataObject) -> Result<Vec<WebView2VirtualFile>>>;

/// Lives with the host subclass, so HWND destruction revokes the OLE target
/// before the HWND can be reused. Never revokes another host's registration.
pub(super) struct Registration {
  hwnd: HWND,
  source: Option<(ICoreWebView2CompositionController5, i64)>,
  registered: bool,
  navigation: Option<(ICoreWebView2, i64)>,
}
impl Registration {
  pub unsafe fn new(
    hwnd: HWND,
    controller: &ICoreWebView2CompositionController,
    handler: Option<SourceHandler>,
  ) -> Result<Self> {
    let target_controller: ICoreWebView2CompositionController3 = controller.cast()?;
    let formats = Formats::register()?;
    OleInitialize(None)?;
    let mut registration = Self {
      hwnd,
      source: None,
      registered: false,
      navigation: None,
    };
    let active: ActiveSource = Rc::new(RefCell::new(None));
    if let Some(handler) = handler {
      let source: ICoreWebView2CompositionController5 = controller.cast()?;
      let state = active.clone();
      let mut token = 0;
      source.add_DragStarting(
        &DragStartingEventHandler::create(Box::new(move |_, args| {
          let args = args.ok_or(E_POINTER)?;
          state.borrow_mut().take();
          let result = (|| {
            let data = unsafe { args.Data()? };
            let files = handler(&data)?;
            if !files.is_empty() {
              virtual_file::install(&data, files, formats)?;
              *state.borrow_mut() = Some(data);
            }
            // In particular, do not call DoDragDrop or claim Handled here.
            unsafe { args.SetHandled(false) }
          })();
          if result.is_err() {
            unsafe {
              args.SetHandled(true)?;
            }
          }
          result
        })),
        &mut token,
      )?;
      registration.source = Some((source, token));
      let webview = controller
        .cast::<ICoreWebView2Controller>()?
        .CoreWebView2()?;
      let state = active.clone();
      webview.add_NavigationStarting(
        &NavigationStartingEventHandler::create(Box::new(move |_, _| {
          state.borrow_mut().take();
          Ok(())
        })),
        &mut token,
      )?;
      registration.navigation = Some((webview, token));
    }
    let target: IDropTarget = BrowserTarget {
      hwnd,
      controller: target_controller,
      active,
      formats,
    }
    .into();
    RegisterDragDrop(hwnd, &target)?;
    registration.registered = true;
    Ok(registration)
  }
}
impl Drop for Registration {
  fn drop(&mut self) {
    unsafe {
      if self.registered {
        let _ = RevokeDragDrop(self.hwnd);
      }
      if let Some((source, token)) = self.source.take() {
        let _ = source.remove_DragStarting(token);
      }
      if let Some((webview, token)) = self.navigation.take() {
        let _ = webview.remove_NavigationStarting(token);
      }
      OleUninitialize();
    }
  }
}

#[implement(IDropTarget)]
struct BrowserTarget {
  hwnd: HWND,
  controller: ICoreWebView2CompositionController3,
  active: ActiveSource,
  formats: Formats,
}
impl BrowserTarget {
  fn point(&self, p: &POINTL) -> POINT {
    let mut p = POINT { x: p.x, y: p.y };
    unsafe {
      let _ = ScreenToClient(self.hwnd, &mut p);
    }
    p
  }
}
impl IDropTarget_Impl for BrowserTarget_Impl {
  fn DragEnter(
    &self,
    data: Ref<IDataObject>,
    keys: MODIFIERKEYS_FLAGS,
    point: &POINTL,
    effect: *mut DROPEFFECT,
  ) -> Result<()> {
    if effect.is_null() {
      return Err(E_POINTER.into());
    }
    let data =
      virtual_file::for_self_drop(data.as_ref().ok_or(E_POINTER)?, &self.active, self.formats)?;
    unsafe {
      self
        .controller
        .DragEnter(&data, keys.0, self.point(point), effect.cast())
    }
  }
  fn DragOver(
    &self,
    keys: MODIFIERKEYS_FLAGS,
    point: &POINTL,
    effect: *mut DROPEFFECT,
  ) -> Result<()> {
    if effect.is_null() {
      return Err(E_POINTER.into());
    }
    unsafe {
      self
        .controller
        .DragOver(keys.0, self.point(point), effect.cast())
    }
  }
  fn DragLeave(&self) -> Result<()> {
    unsafe { self.controller.DragLeave() }
  }
  fn Drop(
    &self,
    data: Ref<IDataObject>,
    keys: MODIFIERKEYS_FLAGS,
    point: &POINTL,
    effect: *mut DROPEFFECT,
  ) -> Result<()> {
    if effect.is_null() {
      return Err(E_POINTER.into());
    }
    let data =
      virtual_file::for_self_drop(data.as_ref().ok_or(E_POINTER)?, &self.active, self.formats)?;
    unsafe {
      self
        .controller
        .Drop(&data, keys.0, self.point(point), effect.cast())
    }
  }
}
