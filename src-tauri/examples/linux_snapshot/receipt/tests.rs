use super::*;

#[test]
fn encoded_size_limit() {
    let mut writer = BoundedWriter::new(8);
    assert!(writer.write_all(b"12345678").is_ok());
    let error = writer
        .write_all(b"9")
        .expect_err("ninth byte exceeds the limit");
    assert_eq!(error.to_string(), "encoded_size_limit");
    assert_eq!(writer.into_bytes(), b"12345678");

    let root = tempfile::tempdir().expect("temp dir");
    let mut receipt = Receipt::create(&root.path().join("out")).expect("new receipt");
    let oversized = vec![0_u8; MAX_PNG_BYTES + 1];
    assert!(receipt.publish_png("big.png", &oversized).is_err());
    assert!(!root.path().join("out").join("big.png").exists());
    assert_eq!(receipt.png_count(), 0);
}

#[test]
fn receipt_no_clobber() {
    let root = tempfile::tempdir().expect("temp dir");
    let out = root.path().join("out");
    let mut receipt = Receipt::create(&out).expect("new receipt");
    assert!(out.join("webkit-target").is_dir());
    assert!(out.join("webkit-control").is_dir());
    assert!(
        Receipt::create(&out).is_err(),
        "existing output directory is refused"
    );

    receipt.publish_png("a.png", b"first").expect("first write");
    assert!(receipt.publish_png("a.png", b"second").is_err());
    assert_eq!(fs::read(out.join("a.png")).expect("read back"), b"first");
    assert!(receipt.publish_png("../escape.png", b"x").is_err());
    assert_eq!(receipt.png_count(), 1);

    fs::write(out.join(RECEIPT_NAME), b"external").expect("pre-existing receipt");
    let code = receipt.finish(&Outcome::Pass);

    assert_eq!(code, 2, "receipt.json is never replaced");
    assert_eq!(
        fs::read(out.join(RECEIPT_NAME)).expect("read back"),
        b"external"
    );
}
