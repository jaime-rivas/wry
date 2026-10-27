// Copyright 2020-2026 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::cell::RefCell;

use block2::Block;
use objc2::{
  define_class, msg_send,
  rc::Retained,
  runtime::{AnyClass, AnyObject, Bool, NSObject, ProtocolObject},
  ClassType, DefinedClass, MainThreadOnly,
};
use objc2_app_kit::{
  NSDragOperation, NSDraggingFormation, NSDraggingInfo, NSDraggingItem,
  NSDraggingItemEnumerationOptions, NSEvent, NSEventModifierFlags, NSImage, NSPasteboard,
  NSPasteboardNameDrag, NSPasteboardReadingOptionKey, NSSpringLoadingHighlight, NSView, NSWindow,
};
use objc2_foundation::{NSArray, NSDictionary, NSObjectProtocol, NSPoint, NSString, NSURL};
use std::ptr::NonNull;

use super::WryWebView;

/// Native items supplied for a custom HTML source drag.
/// Requires the private macOS 27 WebKit source API.
pub struct MacosDragSource {
  /// File-promise items. Their providers must retain their own delegates.
  pub items: Vec<Retained<NSDraggingItem>>,
  /// Operations permitted inside the app. Match the HTML source's effectAllowed.
  /// External destinations retain WebKit's default (copy) operation mask.
  pub local_operations: NSDragOperation,
}
pub(crate) type SourceHandler = dyn Fn(&NSPasteboard, &NSDraggingItem) -> Option<MacosDragSource>;

pub(crate) struct Source {
  pub handler: Option<Box<SourceHandler>>,
  // Keep the original data until the next source gesture or webview destruction.
  // AppKit sends source completion before WebKit delivers the asynchronous DOM drop.
  original: RefCell<Option<PasteboardSnapshot>>,
  local_operations: std::cell::Cell<NSDragOperation>,
}

struct PasteboardSnapshot(Retained<NSPasteboard>);
impl Drop for PasteboardSnapshot {
  fn drop(&mut self) {
    unsafe {
      let _: () = msg_send![&*self.0, releaseGlobally];
    };
  }
}

impl Source {
  pub fn new(handler: Option<Box<SourceHandler>>) -> Self {
    if handler.is_some() {
      let _ = LocalInfo::class();
    }
    Self {
      handler,
      original: RefCell::new(None),
      local_operations: std::cell::Cell::new(NSDragOperation::None),
    }
  }
}

pub(crate) fn start(
  view: &WryWebView,
  placeholder: &NSDraggingItem,
  completion: &Block<dyn Fn(*const NSArray<NSDraggingItem>)>,
) {
  let state = &view.ivars().drag_source;
  state.original.borrow_mut().take();
  let Some(handler) = &state.handler else {
    completion.call((std::ptr::null(),));
    return;
  };
  unsafe {
    let pasteboard = NSPasteboard::pasteboardWithName(NSPasteboardNameDrag);
    let source = handler(&pasteboard, placeholder);
    let Some(source) = source.filter(|source| !source.items.is_empty()) else {
      completion.call((std::ptr::null(),));
      return;
    };
    state.local_operations.set(source.local_operations);
    // Copy browser-produced data, including its origin metadata, verbatim.
    // Never manufacture WebKit's private custom-data representation.
    let local = NSPasteboard::pasteboardWithUniqueName();
    if let Some(types) = pasteboard.types() {
      for ty in types {
        if let Some(data) = pasteboard.dataForType(&ty) {
          local.setData_forType(Some(&data), &ty);
        }
      }
    }
    *state.original.borrow_mut() = Some(PasteboardSnapshot(local));
    let items = NSArray::from_retained_slice(&source.items);
    completion.call((Retained::as_ptr(&items),));
  }
}

struct LocalInfoIvars {
  original: Retained<ProtocolObject<dyn NSDraggingInfo>>,
  pasteboard: Retained<NSPasteboard>,
  operations: NSDragOperation,
}

// A forwarding adapter preserves the native source identity, location, allowed
// effects and lifetime while selecting the source's original HTML pasteboard.
define_class!(
  #[unsafe(super(NSObject))]
  #[thread_kind = MainThreadOnly]
  #[ivars = LocalInfoIvars]
  struct LocalInfo;

  unsafe impl NSObjectProtocol for LocalInfo {}
  unsafe impl NSDraggingInfo for LocalInfo {
    #[unsafe(method_id(draggingPasteboard))]
    fn pasteboard(&self) -> Retained<NSPasteboard> {
      self.ivars().pasteboard.clone()
    }
    #[unsafe(method_id(draggingDestinationWindow))]
    fn window(&self) -> Option<Retained<NSWindow>> {
      self.ivars().original.draggingDestinationWindow()
    }
    #[unsafe(method(draggingSourceOperationMask))]
    fn mask(&self) -> NSDragOperation {
      self.ivars().operations
    }
    #[unsafe(method(draggingLocation))]
    fn location(&self) -> NSPoint {
      self.ivars().original.draggingLocation()
    }
    #[unsafe(method(draggedImageLocation))]
    fn image_location(&self) -> NSPoint {
      self.ivars().original.draggedImageLocation()
    }
    #[allow(deprecated)]
    #[unsafe(method_id(draggedImage))]
    fn image(&self) -> Option<Retained<NSImage>> {
      self.ivars().original.draggedImage()
    }
    #[unsafe(method_id(draggingSource))]
    fn source(&self) -> Option<Retained<AnyObject>> {
      self.ivars().original.draggingSource()
    }
    #[unsafe(method(draggingSequenceNumber))]
    fn sequence(&self) -> isize {
      self.ivars().original.draggingSequenceNumber()
    }
    #[unsafe(method(slideDraggedImageTo:))]
    fn slide(&self, point: NSPoint) {
      self.ivars().original.slideDraggedImageTo(point)
    }
    #[allow(deprecated)]
    #[unsafe(method_id(namesOfPromisedFilesDroppedAtDestination:))]
    fn promised_names(&self, url: &NSURL) -> Option<Retained<NSArray<NSString>>> {
      self
        .ivars()
        .original
        .namesOfPromisedFilesDroppedAtDestination(url)
    }
    #[unsafe(method(draggingFormation))]
    fn formation(&self) -> NSDraggingFormation {
      self.ivars().original.draggingFormation()
    }
    #[unsafe(method(setDraggingFormation:))]
    fn set_formation(&self, value: NSDraggingFormation) {
      self.ivars().original.setDraggingFormation(value)
    }
    #[unsafe(method(animatesToDestination))]
    fn animates(&self) -> bool {
      self.ivars().original.animatesToDestination()
    }
    #[unsafe(method(setAnimatesToDestination:))]
    fn set_animates(&self, value: bool) {
      self.ivars().original.setAnimatesToDestination(value)
    }
    #[unsafe(method(numberOfValidItemsForDrop))]
    fn count(&self) -> isize {
      self.ivars().original.numberOfValidItemsForDrop()
    }
    #[unsafe(method(setNumberOfValidItemsForDrop:))]
    fn set_count(&self, value: isize) {
      self.ivars().original.setNumberOfValidItemsForDrop(value)
    }
    #[unsafe(method(enumerateDraggingItemsWithOptions:forView:classes:searchOptions:usingBlock:))]
    unsafe fn enumerate(
      &self,
      options: NSDraggingItemEnumerationOptions,
      view: Option<&NSView>,
      classes: &NSArray<AnyClass>,
      search: &NSDictionary<NSPasteboardReadingOptionKey, AnyObject>,
      block: &Block<dyn Fn(NonNull<NSDraggingItem>, isize, NonNull<Bool>)>,
    ) {
      self
        .ivars()
        .original
        .enumerateDraggingItemsWithOptions_forView_classes_searchOptions_usingBlock(
          options, view, classes, search, block,
        )
    }
    #[unsafe(method(springLoadingHighlight))]
    fn highlight(&self) -> NSSpringLoadingHighlight {
      self.ivars().original.springLoadingHighlight()
    }
    #[unsafe(method(resetSpringLoading))]
    fn reset_spring(&self) {
      self.ivars().original.resetSpringLoading()
    }
  }
);

pub(crate) fn local_info(
  view: &WryWebView,
  info: &ProtocolObject<dyn NSDraggingInfo>,
) -> Option<Retained<ProtocolObject<dyn NSDraggingInfo>>> {
  unsafe {
    let source = info.draggingSource()?;
    if Retained::as_ptr(&source).cast::<WryWebView>() != view as *const _ {
      return None;
    }
    let state = view.ivars().drag_source.original.borrow();
    let pasteboard = state.as_ref()?.0.clone();
    let original = Retained::retain(info as *const _ as *mut _)?;
    let proxy = view.mtm().alloc::<LocalInfo>().set_ivars(LocalInfoIvars {
      original,
      pasteboard,
      operations: local_operation_mask(
        view.ivars().drag_source.local_operations.get(),
        NSEvent::modifierFlags_class(),
      ),
    });
    let proxy: Retained<LocalInfo> = msg_send![super(proxy), init];
    Some(ProtocolObject::from_retained(proxy))
  }
}

pub(crate) fn operation_mask(
  view: &WryWebView,
  context: objc2_app_kit::NSDraggingContext,
  default: NSDragOperation,
) -> NSDragOperation {
  let state = &view.ivars().drag_source;
  if context == objc2_app_kit::NSDraggingContext::WithinApplication
    && state.original.borrow().is_some()
  {
    state.local_operations.get()
  } else {
    default
  }
}

// AppKit can narrow a promised-file drag to Copy before WebKit sees it, and
// WebKit may omit modifier flags on DOM drag events. For this application's
// original HTML representation, apply native modifiers to the source's declared
// local capabilities, never widening them. Other native sources are untouched.
fn local_operation_mask(allowed: NSDragOperation, flags: NSEventModifierFlags) -> NSDragOperation {
  let option = flags.contains(NSEventModifierFlags::Option);
  let command = flags.contains(NSEventModifierFlags::Command);
  let requested = match (option, command) {
    (true, true) => NSDragOperation::Link,
    (true, false) => NSDragOperation::Copy,
    (false, true) => NSDragOperation::Move,
    (false, false) => return allowed,
  };
  allowed & requested
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn local_modifiers_never_widen_source_capabilities() {
    use NSEventModifierFlags as F;
    let all = NSDragOperation::Copy | NSDragOperation::Move | NSDragOperation::Link;
    assert_eq!(local_operation_mask(all, F::empty()), all);
    assert_eq!(local_operation_mask(all, F::Option), NSDragOperation::Copy);
    assert_eq!(local_operation_mask(all, F::Command), NSDragOperation::Move);
    assert_eq!(
      local_operation_mask(all, F::Option | F::Command),
      NSDragOperation::Link
    );
    assert_eq!(
      local_operation_mask(NSDragOperation::Copy, F::Option | F::Command),
      NSDragOperation::None
    );
    assert_eq!(
      local_operation_mask(NSDragOperation::None, F::empty()),
      NSDragOperation::None
    );
  }
  #[test]
  fn local_info_class_registration() {
    // objc2 verifies every required NSDraggingInfo method during registration.
    // Catch incomplete adapters before the first actual user drag.
    let _ = LocalInfo::class();
  }
}
