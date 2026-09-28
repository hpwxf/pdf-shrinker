//! End-to-end tests that build tiny synthetic PDFs (so we don't need to commit
//! any binary fixtures) and run them through the compression engines.

use image::codecs::jpeg::JpegEncoder;
use image::{ImageBuffer, Rgb};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, EncryptionState, EncryptionVersion, Object, ObjectId, Permissions, Stream, dictionary};
use tempfile::tempdir;

use pdfshrink_core::{CompressOptions, Engine, EngineChoice, Level, Outcome, PdfShrinkError, RustEngine, compress_file};

fn make_jpeg_bytes(w: u32, h: u32, quality: u8) -> Vec<u8> {
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(w, h, |x, y| {
        Rgb([((x * 7) % 256) as u8, ((y * 13) % 256) as u8, (((x + y) * 3) % 256) as u8])
    });
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality)
        .encode_image(&img)
        .unwrap();
    buf
}

fn make_flate_rgb_stream(w: u32, h: u32) -> Stream {
    let mut raw = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            raw.push(((x * 3) % 256) as u8);
            raw.push(((y * 5) % 256) as u8);
            raw.push((((x + y) * 2) % 256) as u8);
        }
    }
    let dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => w as i64,
        "Height" => h as i64,
        "ColorSpace" => "DeviceRGB",
        "BitsPerComponent" => 8,
    };
    let mut stream = Stream::new(dict, raw);
    stream.compress().unwrap();
    stream
}

fn make_gray_smask(w: u32, h: u32) -> Stream {
    let raw: Vec<u8> = (0..(w * h)).map(|i| (i % 256) as u8).collect();
    let dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => w as i64,
        "Height" => h as i64,
        "ColorSpace" => "DeviceGray",
        "BitsPerComponent" => 8,
    };
    let mut stream = Stream::new(dict, raw);
    stream.compress().unwrap();
    stream
}

fn jpeg_image_stream(jpeg: Vec<u8>, w: u32, h: u32) -> Stream {
    let dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => w as i64,
        "Height" => h as i64,
        "ColorSpace" => "DeviceRGB",
        "BitsPerComponent" => 8,
        "Filter" => "DCTDecode",
    };
    Stream::new(dict, jpeg).with_compression(false)
}

/// Builds a minimal one-page PDF with a single image XObject drawn to fill
/// `draw_w` x `draw_h` PDF points on a `page_w` x `page_h` page.
fn build_single_image_pdf(image: Stream, page_w: f64, page_h: f64, draw_w: f64, draw_h: f64) -> (Document, ObjectId) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let img_id = doc.add_object(image);

    let ops = vec![
        Operation::new("q", vec![]),
        Operation::new(
            "cm",
            vec![
                Object::Real(draw_w as f32),
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(draw_h as f32),
                Object::Real(0.0),
                Object::Real(0.0),
            ],
        ),
        Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
        Operation::new("Q", vec![]),
    ];
    let content_bytes = Content { operations: ops }.encode().unwrap();
    let content_id = doc.add_object(Stream::new(Dictionary::new(), content_bytes));

    let resources = dictionary! {
        "XObject" => dictionary! { "Im0" => Object::Reference(img_id) },
    };

    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(pages_id),
        "Contents" => Object::Reference(content_id),
        "Resources" => resources,
        "MediaBox" => vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(page_w as f32),
            Object::Real(page_h as f32),
        ],
    });

    let pages = dictionary! {
        "Type" => "Pages",
        "Count" => 1,
        "Kids" => vec![Object::Reference(page_id)],
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages));

    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    });
    doc.trailer.set("Root", Object::Reference(catalog_id));

    (doc, img_id)
}

/// A text-only, image-free one-page PDF.
fn build_text_only_pdf() -> Document {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();

    let content_bytes = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), Object::Integer(12)]),
            Operation::new(
                "Td",
                vec![Object::Integer(72), Object::Integer(720)],
            ),
            Operation::new("Tj", vec![Object::string_literal("Hello, PdfShrinker!")]),
            Operation::new("ET", vec![]),
        ],
    }
    .encode()
    .unwrap();
    let content_id = doc.add_object(Stream::new(Dictionary::new(), content_bytes));

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources = dictionary! {
        "Font" => dictionary! { "F1" => Object::Reference(font_id) },
    };

    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(pages_id),
        "Contents" => Object::Reference(content_id),
        "Resources" => resources,
        "MediaBox" => vec![Object::Real(0.0), Object::Real(0.0), Object::Real(612.0), Object::Real(792.0)],
    });

    let pages = dictionary! {
        "Type" => "Pages",
        "Count" => 1,
        "Kids" => vec![Object::Reference(page_id)],
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages));

    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    });
    doc.trailer.set("Root", Object::Reference(catalog_id));

    doc
}

fn save_and_size(doc: &mut Document, path: &std::path::Path) -> u64 {
    doc.save(path).unwrap();
    std::fs::metadata(path).unwrap().len()
}

fn reload_image_dims(path: &std::path::Path, img_id: ObjectId) -> (i64, i64) {
    let doc = Document::load(path).unwrap();
    let obj = doc.get_object(img_id).unwrap();
    let stream = obj.as_stream().unwrap();
    (
        stream.dict.get(b"Width").and_then(Object::as_i64).unwrap(),
        stream.dict.get(b"Height").and_then(Object::as_i64).unwrap(),
    )
}

#[test]
fn medium_downsamples_an_oversized_jpeg() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");

    // 1200x1200 px drawn into a 200x200 pt box: 432 dpi, well above Medium's
    // 150 dpi * 1.5 trigger threshold.
    let jpeg = make_jpeg_bytes(1200, 1200, 90);
    let stream = jpeg_image_stream(jpeg, 1200, 1200);
    let (mut doc, img_id) = build_single_image_pdf(stream, 400.0, 400.0, 200.0, 200.0);
    let input_size = save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    let report = RustEngine.compress(&input, &output, &Level::Medium.profile()).unwrap();

    assert_eq!(report.images_resampled, 1);
    assert!(report.output_size < input_size, "expected shrinkage: {} -> {}", input_size, report.output_size);

    let (w, h) = reload_image_dims(&output, img_id);
    assert!(w < 1200 && h < 1200, "image should have been downsampled, got {w}x{h}");

    // Page count and basic structure must survive the round trip.
    let reloaded = Document::load(&output).unwrap();
    assert_eq!(reloaded.get_pages().len(), 1);
}

#[test]
fn lossless_never_touches_image_pixels() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");

    let jpeg = make_jpeg_bytes(1200, 1200, 90);
    let stream = jpeg_image_stream(jpeg, 1200, 1200);
    let (mut doc, img_id) = build_single_image_pdf(stream, 400.0, 400.0, 200.0, 200.0);
    save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    let report = RustEngine.compress(&input, &output, &Level::Lossless.profile()).unwrap();

    assert_eq!(report.images_resampled, 0);
    let (w, h) = reload_image_dims(&output, img_id);
    assert_eq!((w, h), (1200, 1200));
}

#[test]
fn smask_is_resized_alongside_its_parent_image() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");

    let mut main = make_flate_rgb_stream(1000, 1000);
    let smask = make_gray_smask(1000, 1000);

    let (mut doc, img_id) = build_single_image_pdf(Stream::new(Dictionary::new(), vec![]), 300.0, 300.0, 150.0, 150.0);
    // Swap in the real image + smask now that we have a document to add the smask to.
    let smask_id = doc.add_object(smask);
    main.dict.set("SMask", Object::Reference(smask_id));
    *doc.objects.get_mut(&img_id).unwrap() = Object::Stream(main);

    save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    RustEngine.compress(&input, &output, &Level::Medium.profile()).unwrap();

    let reloaded = Document::load(&output).unwrap();
    let main_stream = reloaded.get_object(img_id).unwrap().as_stream().unwrap();
    let smask_ref = main_stream.dict.get(b"SMask").and_then(Object::as_reference).unwrap();
    let smask_stream = reloaded.get_object(smask_ref).unwrap().as_stream().unwrap();

    let main_w = main_stream.dict.get(b"Width").and_then(Object::as_i64).unwrap();
    let main_h = main_stream.dict.get(b"Height").and_then(Object::as_i64).unwrap();
    let mask_w = smask_stream.dict.get(b"Width").and_then(Object::as_i64).unwrap();
    let mask_h = smask_stream.dict.get(b"Height").and_then(Object::as_i64).unwrap();

    assert!(main_w < 1000, "main image should have shrunk, got {main_w}");
    assert_eq!((main_w, main_h), (mask_w, mask_h), "SMask must track its parent's new size");
}

#[test]
fn duplicate_images_are_deduplicated() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");

    let jpeg = make_jpeg_bytes(64, 64, 90);
    let stream_a = jpeg_image_stream(jpeg.clone(), 64, 64);
    let stream_b = jpeg_image_stream(jpeg, 64, 64);

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let img_a = doc.add_object(stream_a);
    let img_b = doc.add_object(stream_b);

    let ops = vec![
        Operation::new("Do", vec![Object::Name(b"ImA".to_vec())]),
        Operation::new("Do", vec![Object::Name(b"ImB".to_vec())]),
    ];
    let content_bytes = Content { operations: ops }.encode().unwrap();
    let content_id = doc.add_object(Stream::new(Dictionary::new(), content_bytes));
    let resources = dictionary! {
        "XObject" => dictionary! { "ImA" => Object::Reference(img_a), "ImB" => Object::Reference(img_b) },
    };
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(pages_id),
        "Contents" => Object::Reference(content_id),
        "Resources" => resources,
        "MediaBox" => vec![Object::Real(0.0), Object::Real(0.0), Object::Real(100.0), Object::Real(100.0)],
    });
    let pages = dictionary! { "Type" => "Pages", "Count" => 1, "Kids" => vec![Object::Reference(page_id)] };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(pages_id) });
    doc.trailer.set("Root", Object::Reference(catalog_id));

    save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    RustEngine.compress(&input, &output, &Level::Lossless.profile()).unwrap();

    let reloaded = Document::load(&output).unwrap();
    let image_count = reloaded
        .objects
        .values()
        .filter(|o| matches!(o, Object::Stream(s) if s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image".as_slice())))
        .count();
    assert_eq!(image_count, 1, "identical image streams should have been merged into one object");
}

#[test]
fn text_only_pdf_round_trips() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");
    let mut doc = build_text_only_pdf();
    save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    let report = RustEngine.compress(&input, &output, &Level::Medium.profile()).unwrap();
    assert_eq!(report.images_resampled, 0);

    let reloaded = Document::load(&output).unwrap();
    assert_eq!(reloaded.get_pages().len(), 1);
}

#[test]
fn encrypted_pdf_is_rejected() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");
    let mut doc = build_text_only_pdf();
    doc.trailer.set(
        "ID",
        vec![
            Object::string_literal("0123456789ABCDEF"),
            Object::string_literal("0123456789ABCDEF"),
        ],
    );
    let version = EncryptionVersion::V1 {
        document: &doc,
        owner_password: "owner",
        user_password: "user",
        permissions: Permissions::default(),
    };
    let state = EncryptionState::try_from(version).unwrap();
    doc.encrypt(&state).unwrap();
    save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    let err = RustEngine.compress(&input, &output, &Level::Medium.profile()).unwrap_err();
    assert!(matches!(err, PdfShrinkError::Encrypted(_)), "expected Encrypted, got {err:?}");
}

#[test]
fn facade_names_output_and_reports_not_smaller_when_nothing_to_gain() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("report.pdf");
    let mut doc = build_text_only_pdf();
    save_and_size(&mut doc, &input);

    let opts = CompressOptions {
        level: Level::Lossless,
        engine: EngineChoice::Rust,
    };

    match compress_file(&input, &opts).unwrap() {
        Outcome::Compressed { output, .. } => {
            assert_eq!(output.file_name().unwrap().to_str().unwrap(), "report-compressed.pdf");
        }
        Outcome::NotSmaller => {
            // A trivial text-only PDF may already be as small as lopdf can make
            // it; that's an acceptable outcome too, and no file must exist.
            assert!(!dir.path().join("report-compressed.pdf").exists());
        }
    }
}
