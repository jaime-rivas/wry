// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT
use super::*;
use std::sync::atomic::AtomicUsize;
use windows::core::Interface;

fn fixture(calls: Arc<AtomicUsize>) -> WebView2VirtualFile {
  WebView2VirtualFile {
    name: "deferred.txt".into(),
    size: 6,
    read_at: Arc::new(move |offset, out| {
      calls.fetch_add(1, Ordering::Relaxed);
      out.copy_from_slice(&b"abcdef"[offset as usize..offset as usize + out.len()]);
      Ok(out.len())
    }),
  }
}
fn stream(file: WebView2VirtualFile) -> IStream {
  LazyStream {
    session: Arc::new(Session {
      files: vec![file],
      ready: AtomicBool::new(true),
    }),
    index: 0,
    position: Mutex::new(0),
  }
  .into()
}
unsafe fn read(stream: &IStream, bytes: &mut [u8]) -> (HRESULT, u32) {
  let mut count = 0;
  (
    stream.Read(
      bytes.as_mut_ptr().cast(),
      bytes.len() as u32,
      Some(&mut count),
    ),
    count,
  )
}

#[test]
fn metadata_seek_and_clone_do_not_read_content() {
  unsafe {
    let calls = Arc::new(AtomicUsize::new(0));
    let a = stream(fixture(calls.clone()));
    let mut stat = STATSTG::default();
    a.Stat(&mut stat, STATFLAG_NONAME).unwrap();
    assert_eq!(stat.cbSize, 6);
    a.Seek(2, STREAM_SEEK_SET, None).unwrap();
    let b = a.Clone().unwrap();
    assert_eq!(a.Read(ptr::null_mut(), 0, None), S_OK);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    let mut out = [0; 2];
    assert_eq!(read(&b, &mut out), (S_OK, 2));
    assert_eq!(&out, b"cd");
    assert_eq!(read(&a, &mut out), (S_OK, 2));
    assert_eq!(&out, b"cd");
    b.Seek(-1, STREAM_SEEK_END, None).unwrap();
    assert_eq!(read(&b, &mut out), (S_FALSE, 1));
    assert_eq!(out[0], b'f');
    assert_eq!(read(&b, &mut out), (S_FALSE, 0));
    assert_eq!(calls.load(Ordering::Relaxed), 3);
  }
}

#[test]
fn invalid_seek_preserves_position_and_reader_errors_propagate() {
  unsafe {
    let a = stream(WebView2VirtualFile {
      name: "failure.txt".into(),
      size: 4,
      read_at: Arc::new(|_, _| Err(E_ACCESSDENIED.into())),
    });
    a.Seek(2, STREAM_SEEK_SET, None).unwrap();
    assert!(a.Seek(i64::MIN, STREAM_SEEK_CUR, None).is_err());
    let mut position = 0;
    a.Seek(0, STREAM_SEEK_CUR, Some(&mut position)).unwrap();
    assert_eq!(position, 2);
    assert_eq!(read(&a, &mut [0; 2]), (E_ACCESSDENIED, 0));
    let a = stream(WebView2VirtualFile {
      name: "bad.txt".into(),
      size: 4,
      read_at: Arc::new(|_, out| Ok(out.len() + 1)),
    });
    assert_eq!(read(&a, &mut [0; 4]), (E_UNEXPECTED, 0));
    let a = stream(WebView2VirtualFile {
      name: "short.txt".into(),
      size: 4,
      read_at: Arc::new(|_, _| Ok(0)),
    });
    assert_eq!(read(&a, &mut [0; 4]), (STG_E_READFAULT, 0));
  }
}

#[test]
fn virtual_names_cannot_escape_the_destination() {
  for name in [
    "",
    ".",
    "..",
    "../x",
    "a/b",
    "a\\b",
    "C:x",
    "a\0b",
    "x.",
    "x ",
    "CON.txt",
    "lpt1",
    "COM².log",
  ] {
    assert!(!valid_name(name), "{name:?}");
  }
  assert!(valid_name("résumé.tar.gz"));
  assert!(!valid_name(&"a".repeat(260)));
}

#[test]
fn shell_object_installation_preserves_text_and_stream_outlives_source() {
  unsafe {
    OleInitialize(None).unwrap();
    {
      let original: IDataObject = SHCreateDataObject(None, None, None).unwrap();
      let text = format(CF_UNICODETEXT.0, TYMED_HGLOBAL, -1);
      let memory = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 4).unwrap();
      let p = GlobalLock(memory).cast::<u16>();
      assert!(!p.is_null());
      p.write(b'A' as u16);
      let _ = GlobalUnlock(memory);
      let medium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: memory },
        pUnkForRelease: ManuallyDrop::new(None),
      };
      original.SetData(&text, &medium, true).unwrap();
      let formats = Formats::register().unwrap();
      let calls = Arc::new(AtomicUsize::new(0));
      let lifetime = install(&original, vec![fixture(calls.clone())], formats).unwrap();
      assert!(lifetime.upgrade().is_some());
      assert_eq!(calls.load(Ordering::Relaxed), 0);
      assert_eq!(
        original.QueryGetData(&format(formats.ignore_contents, TYMED_HGLOBAL, -1)),
        S_OK
      );
      let content = format(formats.contents, TYMED_ISTREAM, 0);
      let mut medium = original.GetData(&text).unwrap();
      let p = GlobalLock(medium.u.hGlobal).cast::<u16>();
      assert!(!p.is_null());
      assert_eq!(*p, b'A' as u16);
      let _ = GlobalUnlock(medium.u.hGlobal);
      ReleaseStgMedium(&mut medium);
      let mut medium = original.GetData(&content).unwrap();
      let stream = medium.u.pstm.as_ref().unwrap().clone();
      ReleaseStgMedium(&mut medium);
      drop(original);
      let mut out = [0; 6];
      assert_eq!(read(&stream, &mut out), (S_OK, 6));
      assert_eq!(&out, b"abcdef");
      assert_eq!(calls.load(Ordering::Relaxed), 1);
      drop(stream);
      assert!(lifetime.upgrade().is_none());
    }
    OleUninitialize();
  }
}

#[test]
fn delayed_stream_can_marshal_to_another_process_without_reading() {
  use windows::Win32::System::Com::{Marshal::*, StructuredStorage::CreateStreamOnHGlobal};
  unsafe {
    OleInitialize(None).unwrap();
    {
      let calls = Arc::new(AtomicUsize::new(0));
      let source = stream(fixture(calls.clone()));
      assert!(source.cast::<IAgileObject>().is_err());
      assert!(
        source.cast::<IMarshal>().is_err(),
        "Use the standard COM marshaler like the working classic-COM probe"
      );
      let packet = CreateStreamOnHGlobal(HGLOBAL::default(), true).unwrap();
      let result = CoMarshalInterface(
        &packet,
        &IStream::IID,
        &source,
        MSHCTX_LOCAL.0 as u32,
        None,
        MSHLFLAGS_NORMAL.0 as u32,
      );
      assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "Marshaling must not materialize content"
      );
      result.expect("External COM marshaling must succeed without a read fallback");
      packet.Seek(0, STREAM_SEEK_SET, None).unwrap();
      let receiver: IStream = CoUnmarshalInterface(&packet).unwrap();
      assert_eq!(calls.load(Ordering::Relaxed), 0);
      assert_eq!(read(&receiver, &mut [0; 6]), (S_OK, 6));
    }
    OleUninitialize();
  }
}

#[test]
fn multiple_virtual_files_keep_separate_streams_and_reject_collisions() {
  unsafe {
    OleInitialize(None).unwrap();
    {
      let original: IDataObject = SHCreateDataObject(None, None, None).unwrap();
      let formats = Formats::register().unwrap();
      let calls = Arc::new(AtomicUsize::new(0));
      let a = fixture(calls.clone());
      let mut b = fixture(calls.clone());
      b.name = "second.txt".into();
      install(&original, vec![a, b], formats).unwrap();
      assert_eq!(calls.load(Ordering::Relaxed), 0);
      assert!(install(&original, vec![fixture(calls.clone())], formats).is_err());
      for index in 0..2 {
        let mut medium = original
          .GetData(&format(formats.contents, TYMED_ISTREAM, index))
          .unwrap();
        let mut out = [0; 6];
        assert_eq!(read(medium.u.pstm.as_ref().unwrap(), &mut out), (S_OK, 6));
        assert_eq!(&out, b"abcdef");
        ReleaseStgMedium(&mut medium);
      }
      assert_eq!(calls.load(Ordering::Relaxed), 2);
    }
    OleUninitialize();
  }
}
