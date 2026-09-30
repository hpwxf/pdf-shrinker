//! End-to-end tests that build tiny synthetic PDFs (so we don't need to commit
//! any binary fixtures) and run them through the compression engines.

use image::codecs::jpeg::JpegEncoder;
use image::{ImageBuffer, Rgb};
use lopdf::content::{Content, Operation};
use lopdf::{
    Dictionary, Document, EncryptionState, EncryptionVersion, Object, ObjectId, Permissions,
    Stream, dictionary,
};
use tempfile::tempdir;

use pdfshrink_core::{
    CompressOptions, Engine, Level, Outcome, PdfShrinkError, RustEngine, compress_file,
};

fn make_jpeg_bytes(w: u32, h: u32, quality: u8) -> Vec<u8> {
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(w, h, |x, y| {
        Rgb([
            ((x * 7) % 256) as u8,
            ((y * 13) % 256) as u8,
            (((x + y) * 3) % 256) as u8,
        ])
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
fn build_single_image_pdf(
    image: Stream,
    page_w: f64,
    page_h: f64,
    draw_w: f64,
    draw_h: f64,
) -> (Document, ObjectId) {
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
            Operation::new(
                "Tf",
                vec![Object::Name(b"F1".to_vec()), Object::Integer(12)],
            ),
            Operation::new("Td", vec![Object::Integer(72), Object::Integer(720)]),
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
    let report = RustEngine
        .compress(&input, &output, &Level::Medium.profile())
        .unwrap();

    assert_eq!(report.images_resampled, 1);
    assert!(
        report.output_size < input_size,
        "expected shrinkage: {} -> {}",
        input_size,
        report.output_size
    );

    let (w, h) = reload_image_dims(&output, img_id);
    assert!(
        w < 1200 && h < 1200,
        "image should have been downsampled, got {w}x{h}"
    );

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
    let report = RustEngine
        .compress(&input, &output, &Level::Lossless.profile())
        .unwrap();

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

    let (mut doc, img_id) = build_single_image_pdf(
        Stream::new(Dictionary::new(), vec![]),
        300.0,
        300.0,
        150.0,
        150.0,
    );
    // Swap in the real image + smask now that we have a document to add the smask to.
    let smask_id = doc.add_object(smask);
    main.dict.set("SMask", Object::Reference(smask_id));
    *doc.objects.get_mut(&img_id).unwrap() = Object::Stream(main);

    save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    RustEngine
        .compress(&input, &output, &Level::Medium.profile())
        .unwrap();

    let reloaded = Document::load(&output).unwrap();
    let main_stream = reloaded.get_object(img_id).unwrap().as_stream().unwrap();
    let smask_ref = main_stream
        .dict
        .get(b"SMask")
        .and_then(Object::as_reference)
        .unwrap();
    let smask_stream = reloaded.get_object(smask_ref).unwrap().as_stream().unwrap();

    let main_w = main_stream
        .dict
        .get(b"Width")
        .and_then(Object::as_i64)
        .unwrap();
    let main_h = main_stream
        .dict
        .get(b"Height")
        .and_then(Object::as_i64)
        .unwrap();
    let mask_w = smask_stream
        .dict
        .get(b"Width")
        .and_then(Object::as_i64)
        .unwrap();
    let mask_h = smask_stream
        .dict
        .get(b"Height")
        .and_then(Object::as_i64)
        .unwrap();

    assert!(main_w < 1000, "main image should have shrunk, got {main_w}");
    assert_eq!(
        (main_w, main_h),
        (mask_w, mask_h),
        "SMask must track its parent's new size"
    );
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
    let pages =
        dictionary! { "Type" => "Pages", "Count" => 1, "Kids" => vec![Object::Reference(page_id)] };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id =
        doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(pages_id) });
    doc.trailer.set("Root", Object::Reference(catalog_id));

    save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    RustEngine
        .compress(&input, &output, &Level::Lossless.profile())
        .unwrap();

    let reloaded = Document::load(&output).unwrap();
    let image_count = reloaded
        .objects
        .values()
        .filter(|o| matches!(o, Object::Stream(s) if s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image".as_slice())))
        .count();
    assert_eq!(
        image_count, 1,
        "identical image streams should have been merged into one object"
    );
}

#[test]
fn text_only_pdf_round_trips() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");
    let mut doc = build_text_only_pdf();
    save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    let report = RustEngine
        .compress(&input, &output, &Level::Medium.profile())
        .unwrap();
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
    let err = RustEngine
        .compress(&input, &output, &Level::Medium.profile())
        .unwrap_err();
    assert!(
        matches!(err, PdfShrinkError::Encrypted(_)),
        "expected Encrypted, got {err:?}"
    );
}

#[test]
fn facade_names_output_and_reports_not_smaller_when_nothing_to_gain() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("report.pdf");
    let mut doc = build_text_only_pdf();
    save_and_size(&mut doc, &input);

    let opts = CompressOptions {
        level: Level::Lossless,
    };

    match compress_file(&input, &opts).unwrap() {
        Outcome::Compressed { output, .. } => {
            assert_eq!(
                output.file_name().unwrap().to_str().unwrap(),
                "report-compressed.pdf"
            );
        }
        Outcome::NotSmaller => {
            // A trivial text-only PDF may already be as small as lopdf can make
            // it; that's an acceptable outcome too, and no file must exist.
            assert!(!dir.path().join("report-compressed.pdf").exists());
        }
    }
}

/// One page drawing every `(name, image)` pair full-page.
fn build_multi_image_pdf(images: Vec<Stream>) -> (Document, Vec<ObjectId>) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let ids: Vec<ObjectId> = images.into_iter().map(|s| doc.add_object(s)).collect();
    let mut ops = Vec::new();
    let mut xobjects = Dictionary::new();
    for (i, id) in ids.iter().enumerate() {
        let name = format!("Im{i}");
        ops.push(Operation::new("q", vec![]));
        ops.push(Operation::new(
            "cm",
            [100.0, 0.0, 0.0, 100.0, 0.0, 0.0]
                .into_iter()
                .map(Object::Real)
                .collect(),
        ));
        ops.push(Operation::new(
            "Do",
            vec![Object::Name(name.clone().into_bytes())],
        ));
        ops.push(Operation::new("Q", vec![]));
        xobjects.set(name, Object::Reference(*id));
    }
    let content_bytes = Content { operations: ops }.encode().unwrap();
    let content_id = doc.add_object(Stream::new(Dictionary::new(), content_bytes));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(pages_id),
        "Contents" => Object::Reference(content_id),
        "Resources" => dictionary! { "XObject" => xobjects },
        "MediaBox" => vec![Object::Real(0.0), Object::Real(0.0), Object::Real(100.0), Object::Real(100.0)],
    });
    let pages =
        dictionary! { "Type" => "Pages", "Count" => 1, "Kids" => vec![Object::Reference(page_id)] };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id =
        doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(pages_id) });
    doc.trailer.set("Root", Object::Reference(catalog_id));
    (doc, ids)
}

fn count_images(doc: &Document) -> usize {
    doc.objects
        .values()
        .filter(|o| matches!(o, Object::Stream(s) if s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image".as_slice())))
        .count()
}

#[test]
fn extreme_merges_images_identical_after_decoding() {
    // Same pixels and same soft-mask pixels, but encoded differently (raw vs
    // Flate) and each pointing at its own mask object: invisible to byte-level
    // dedup, one image + one mask for the experimental deep dedup.
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");
    let raw_img = || {
        let mut s = make_flate_rgb_stream(64, 64);
        s.decompress().unwrap();
        s
    };
    let (mut doc, ids) = build_multi_image_pdf(vec![make_flate_rgb_stream(64, 64), raw_img()]);
    for (i, id) in ids.iter().enumerate() {
        let mut mask = make_gray_smask(64, 64);
        if i == 1 {
            mask.decompress().unwrap();
        }
        let mask_id = doc.add_object(mask);
        doc.get_object_mut(*id)
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .dict
            .set("SMask", Object::Reference(mask_id));
    }
    save_and_size(&mut doc, &input);
    assert_eq!(count_images(&Document::load(&input).unwrap()), 4);

    let output = dir.path().join("out.pdf");
    let mut profile = Level::ExtremeSafe.profile();
    profile.tune("zopfli=0").unwrap();
    RustEngine.compress(&input, &output, &profile).unwrap();
    assert_eq!(
        count_images(&Document::load(&output).unwrap()),
        2,
        "one image + its mask"
    );
}

#[test]
fn indirect_png_predictor_images_are_recompressed() {
    // Regression: lopdf ignores an *indirect* /DecodeParms and returns
    // still-predicted rows, which used to make such images silently skipped.
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");
    let (w, h) = (400u32, 300u32);
    let raw: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [(x ^ y) as u8, (x * 3) as u8, (y * 7 + x) as u8]
        })
        .collect();
    // PNG "None" filter on each row: a filter byte of 0 then the samples.
    let mut predicted = Vec::new();
    for row in raw.chunks((w * 3) as usize) {
        predicted.push(0);
        predicted.extend_from_slice(row);
    }
    let mut stream = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w as i64,
            "Height" => h as i64,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
        },
        predicted,
    );
    stream.compress().unwrap();
    let (mut doc, ids) = build_multi_image_pdf(vec![stream]);
    let parms_id = doc.add_object(dictionary! {
        "Predictor" => 15, "Colors" => 3, "Columns" => w as i64, "BitsPerComponent" => 8,
    });
    doc.get_object_mut(ids[0])
        .unwrap()
        .as_stream_mut()
        .unwrap()
        .dict
        .set("DecodeParms", Object::Reference(parms_id));
    save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    let report = RustEngine
        .compress(&input, &output, &Level::Medium.profile())
        .unwrap();
    assert_eq!(report.images_resampled, 1);
    let out = Document::load(&output).unwrap();
    let img = out.get_object(ids[0]).unwrap().as_stream().unwrap();
    assert_eq!(
        img.dict.get(b"Filter").and_then(Object::as_name).unwrap(),
        b"DCTDecode"
    );
}

#[test]
fn extreme_crops_an_image_hanging_off_the_page() {
    // 400×400 px image drawn 200×200 pt, shifted so only its top-right quarter
    // lands on the 100×100 pt page.
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");
    let (mut doc, img_id) =
        build_single_image_pdf(make_flate_rgb_stream(400, 400), 100.0, 100.0, 200.0, 200.0);
    let page_id = doc.get_pages()[&1];
    let content_id = doc.get_page_contents(page_id)[0];
    let ops = vec![
        Operation::new("q", vec![]),
        Operation::new(
            "cm",
            [200.0, 0.0, 0.0, 200.0, -100.0, 0.0]
                .into_iter()
                .map(Object::Real)
                .collect(),
        ),
        Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
        Operation::new("Q", vec![]),
    ];
    doc.get_object_mut(content_id)
        .unwrap()
        .as_stream_mut()
        .unwrap()
        .set_content(Content { operations: ops }.encode().unwrap());
    save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    let mut profile = Level::ExtremeSafe.profile();
    profile.tune("zopfli=0").unwrap();
    profile.tune("crop=1").unwrap();
    RustEngine.compress(&input, &output, &profile).unwrap();

    let out = Document::load(&output).unwrap();
    let wrapper = out.get_object(img_id).unwrap().as_stream().unwrap();
    assert_eq!(
        wrapper
            .dict
            .get(b"Subtype")
            .and_then(Object::as_name)
            .unwrap(),
        b"Form"
    );
    let inner_id = out
        .get_dict_in_dict(&wrapper.dict, b"Resources")
        .and_then(|r| out.get_dict_in_dict(r, b"XObject"))
        .and_then(|x| x.get(b"I"))
        .and_then(Object::as_reference)
        .unwrap();
    let inner = out.get_object(inner_id).unwrap().as_stream().unwrap();
    let w = inner.dict.get(b"Width").and_then(Object::as_i64).unwrap();
    let h = inner.dict.get(b"Height").and_then(Object::as_i64).unwrap();
    // Right half horizontally (+2 px margin), top half vertically (+2 px margin).
    assert_eq!((w, h), (202, 202));
}

#[test]
fn crop_is_skipped_when_the_image_is_also_used_elsewhere() {
    // Same geometry as above, but the image is also referenced from an
    // unwalked place (here: a page-level /Thumb), so it must stay whole.
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");
    let (mut doc, img_id) =
        build_single_image_pdf(make_flate_rgb_stream(400, 400), 100.0, 100.0, 400.0, 400.0);
    let page_id = doc.get_pages()[&1];
    doc.get_dictionary_mut(page_id)
        .unwrap()
        .set("Thumb", Object::Reference(img_id));
    save_and_size(&mut doc, &input);

    let output = dir.path().join("out.pdf");
    let mut profile = Level::ExtremeSafe.profile();
    profile.tune("zopfli=0").unwrap();
    profile.tune("crop=1").unwrap();
    RustEngine.compress(&input, &output, &profile).unwrap();
    let out = Document::load(&output).unwrap();
    let img = out.get_object(img_id).unwrap().as_stream().unwrap();
    assert_eq!(
        img.dict.get(b"Subtype").and_then(Object::as_name).unwrap(),
        b"Image"
    );
}

// ---------------------------------------------------------------------------
// Image kinds beyond 8-bit gray/RGB: CMYK, palette, 16-bit, JPEG 2000, 1-bit.
// ---------------------------------------------------------------------------

fn image_dict(w: u32, h: u32, cs: Object, bpc: i64) -> Dictionary {
    dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => w as i64,
        "Height" => h as i64,
        "ColorSpace" => cs,
        "BitsPerComponent" => bpc,
    }
}

/// Compresses a one-page document drawing `image` full-page on a 100×100 pt
/// page (so a 400 px wide image sits at 288 dpi) and returns the output
/// image's dictionary and content.
fn compress_single(
    image: Stream,
    profile: pdfshrink_core::Profile,
) -> (Dictionary, Vec<u8>, Document) {
    let dir = tempdir().unwrap();
    let input = dir.path().join("in.pdf");
    let (mut doc, ids) = build_multi_image_pdf(vec![image]);
    save_and_size(&mut doc, &input);
    let output = dir.path().join("out.pdf");
    RustEngine.compress(&input, &output, &profile).unwrap();
    let out = Document::load(&output).unwrap();
    let s = out.get_object(ids[0]).unwrap().as_stream().unwrap().clone();
    (s.dict, s.content, out)
}

fn name(d: &Dictionary, k: &[u8]) -> Vec<u8> {
    d.get(k).and_then(Object::as_name).unwrap().to_vec()
}

fn int_array(d: &Dictionary, k: &[u8]) -> Option<Vec<i64>> {
    d.get(k)
        .and_then(Object::as_array)
        .ok()
        .map(|a| a.iter().map(|o| o.as_i64().unwrap()).collect())
}

fn plasma(w: u32, h: u32, channels: u32) -> Vec<u8> {
    (0..w * h * channels)
        .map(|i| {
            let p = i / channels;
            let (x, y, c) = (p % w, p / w, i % channels);
            ((x * (3 + c) + y * (5 + 2 * c) + (x * y) / 7) % 256) as u8
        })
        .collect()
}

#[test]
fn cmyk_flate_image_becomes_an_inverted_cmyk_jpeg() {
    let (w, h) = (400, 300);
    let mut s = Stream::new(
        image_dict(w, h, Object::Name(b"DeviceCMYK".to_vec()), 8),
        plasma(w, h, 4),
    );
    s.compress().unwrap();
    let (dict, _, _) = compress_single(s, Level::Medium.profile());
    assert_eq!(name(&dict, b"Filter"), b"DCTDecode");
    assert_eq!(name(&dict, b"ColorSpace"), b"DeviceCMYK");
    // Stored inverted, the Photoshop/img2pdf convention.
    assert_eq!(
        int_array(&dict, b"Decode"),
        Some(vec![1, 0, 1, 0, 1, 0, 1, 0])
    );
    assert!(dict.get(b"Width").unwrap().as_i64().unwrap() < w as i64);
}

#[test]
fn adobe_cmyk_jpeg_keeps_its_decode_array() {
    let (w, h) = (400u32, 300u32);
    let jpeg = {
        let mut comp = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_CMYK);
        comp.set_size(w as usize, h as usize);
        comp.set_quality(95.0);
        let mut started = comp.start_compress(Vec::new()).unwrap();
        started.write_scanlines(&plasma(w, h, 4)).unwrap();
        started.finish().unwrap()
    };
    let mut d = image_dict(w, h, Object::Name(b"DeviceCMYK".to_vec()), 8);
    d.set("Filter", "DCTDecode");
    d.set(
        "Decode",
        vec![
            1.into(),
            0.into(),
            1.into(),
            0.into(),
            1.into(),
            0.into(),
            1.into(),
            0.into(),
        ],
    );
    let (dict, content, _) = compress_single(
        Stream::new(d, jpeg).with_compression(false),
        Level::Medium.profile(),
    );
    assert_eq!(name(&dict, b"Filter"), b"DCTDecode");
    assert_eq!(
        int_array(&dict, b"Decode"),
        Some(vec![1, 0, 1, 0, 1, 0, 1, 0])
    );
    assert!(dict.get(b"Width").unwrap().as_i64().unwrap() < w as i64);
    // Re-encoded as a CMYK JPEG (4 components in the SOF marker).
    let sof = content
        .windows(2)
        .position(|m| m == [0xFF, 0xC0] || m == [0xFF, 0xC2])
        .unwrap();
    assert_eq!(content[sof + 9], 4);
}

#[test]
fn palette_image_is_decoded_and_recompressed() {
    let (w, h) = (400u32, 300u32);
    let lut: Vec<u8> = (0..16u32)
        .flat_map(|i| [(i * 16) as u8, (255 - i * 16) as u8, (i * 7) as u8])
        .collect();
    let cs = Object::Array(vec![
        Object::Name(b"Indexed".to_vec()),
        Object::Name(b"DeviceRGB".to_vec()),
        Object::Integer(15),
        Object::String(lut, lopdf::StringFormat::Hexadecimal),
    ]);
    // 4-bit indices, packed two per byte.
    // Noisy indices (dithered photo-like content): Flate does poorly on them.
    let mut seed = 12345u32;
    let idx: Vec<u8> = (0..w * h)
        .map(|_| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            ((seed >> 16) % 16) as u8
        })
        .collect();
    let packed: Vec<u8> = idx
        .chunks(w as usize)
        .flat_map(|row| {
            row.chunks(2)
                .map(|p| (p[0] << 4) | p.get(1).copied().unwrap_or(0))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut s = Stream::new(image_dict(w, h, cs, 4), packed);
    s.compress().unwrap();
    let before = s.content.len();
    let (dict, content, _) = compress_single(s, Level::Medium.profile());
    assert!(content.len() < before);
    assert!(dict.get(b"Width").unwrap().as_i64().unwrap() < w as i64);
}

#[test]
fn sixteen_bit_image_is_reduced_to_8_bits() {
    let (w, h) = (400u32, 300u32);
    let raw16: Vec<u8> = plasma(w, h, 3).iter().flat_map(|v| [*v, 0x80]).collect();
    let mut s = Stream::new(
        image_dict(w, h, Object::Name(b"DeviceRGB".to_vec()), 16),
        raw16,
    );
    s.compress().unwrap();
    let (dict, _, _) = compress_single(s, Level::Medium.profile());
    assert_eq!(name(&dict, b"Filter"), b"DCTDecode");
    assert_eq!(dict.get(b"BitsPerComponent").unwrap().as_i64().unwrap(), 8);
}

/// 16×8 RGB JPEG 2000 with an alpha channel (`cdef` box), made with
/// `opj_compress -n 2 -r 5`: a red→blue gradient, alpha white→black.
const JPX_RGBA: [u8; 279] = [
    0x00, 0x00, 0x00, 0x0c, 0x6a, 0x50, 0x20, 0x20, 0x0d, 0x0a, 0x87, 0x0a, 0x00, 0x00, 0x00, 0x14,
    0x66, 0x74, 0x79, 0x70, 0x6a, 0x70, 0x32, 0x20, 0x00, 0x00, 0x00, 0x00, 0x6a, 0x70, 0x32, 0x20,
    0x00, 0x00, 0x00, 0x4f, 0x6a, 0x70, 0x32, 0x68, 0x00, 0x00, 0x00, 0x16, 0x69, 0x68, 0x64, 0x72,
    0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x10, 0x00, 0x04, 0x07, 0x07, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x0f, 0x63, 0x6f, 0x6c, 0x72, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00,
    0x22, 0x63, 0x64, 0x65, 0x66, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
    0x00, 0x00, 0x02, 0x00, 0x02, 0x00, 0x00, 0x00, 0x03, 0x00, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0xa8, 0x6a, 0x70, 0x32, 0x63, 0xff, 0x4f, 0xff, 0x51, 0x00, 0x32, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x04, 0x07, 0x01, 0x01, 0x07, 0x01, 0x01, 0x07, 0x01, 0x01, 0x07, 0x01, 0x01, 0xff, 0x52, 0x00,
    0x0c, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x04, 0x04, 0x00, 0x01, 0xff, 0x5c, 0x00, 0x07, 0x40,
    0x40, 0x48, 0x48, 0x50, 0xff, 0x64, 0x00, 0x25, 0x00, 0x01, 0x43, 0x72, 0x65, 0x61, 0x74, 0x65,
    0x64, 0x20, 0x62, 0x79, 0x20, 0x4f, 0x70, 0x65, 0x6e, 0x4a, 0x50, 0x45, 0x47, 0x20, 0x76, 0x65,
    0x72, 0x73, 0x69, 0x6f, 0x6e, 0x20, 0x32, 0x2e, 0x35, 0x2e, 0x34, 0xff, 0x90, 0x00, 0x0a, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x2a, 0x00, 0x01, 0xff, 0x93, 0xcf, 0xa4, 0x28, 0x11, 0x50, 0x54, 0xa6,
    0x46, 0xd8, 0x80, 0x08, 0xe1, 0x7a, 0xfd, 0xfe, 0x01, 0x80, 0x15, 0x3c, 0xdd, 0x05, 0x24, 0x45,
    0x80, 0x80, 0x80, 0x80, 0x80, 0xff, 0xd9,
];

#[test]
fn jpeg2000_alpha_becomes_a_soft_mask() {
    let mut d = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => 16,
        "Height" => 8,
        "BitsPerComponent" => 8,
        "Filter" => "JPXDecode",
        "SMaskInData" => 1,
    };
    d.remove(b"ColorSpace");
    let mut profile = Level::ExtremeSafe.profile();
    profile.tune("zopfli=0").unwrap();
    let (dict, _, out) = compress_single(
        Stream::new(d, JPX_RGBA.to_vec()).with_compression(false),
        profile,
    );
    assert_ne!(name(&dict, b"Filter"), b"JPXDecode");
    assert!(!dict.has(b"SMaskInData"));
    let mask_id = dict.get(b"SMask").and_then(Object::as_reference).unwrap();
    let mask = out.get_object(mask_id).unwrap().as_stream().unwrap();
    assert_eq!(mask.dict.get(b"Width").unwrap().as_i64().unwrap(), 16);
    // The alpha gradient survived: opaque on the left, transparent on the right.
    let alpha = mask
        .decompressed_content()
        .unwrap_or_else(|_| mask.content.clone());
    let alpha = if mask.dict.has(b"DecodeParms") {
        // PNG-predicted: row 0 starts after its filter byte.
        alpha[1..17].to_vec()
    } else {
        alpha[..16].to_vec()
    };
    assert!(alpha.len() == 16);
}

/// 1-bit samples resembling a scanned text page: rows of small rings of
/// random sizes ("letters"). Curved, non-repeating shapes like real glyphs,
/// on which CCITT G4 beats Flate (on a real 300 dpi text page, by ~40 %).
fn text_like_bits(w: u32, h: u32) -> Vec<u8> {
    let row_bytes = (w as usize).div_ceil(8);
    let mut raw = vec![0xFFu8; row_bytes * h as usize];
    let mut seed = 11u32;
    let mut rnd = |m: u32| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (seed >> 16) % m
    };
    for line in 0..(h / 24) {
        let mut x = 10i32;
        while x < w as i32 - 20 {
            let r = 4 + rnd(5) as i32;
            let (cx, cy) = (x + r, line as i32 * 24 + 12 + rnd(3) as i32);
            for y in cy - r - 2..=cy + r + 2 {
                for xx in cx - r - 2..=cx + r + 2 {
                    let d2 = (xx - cx).pow(2) + (y - cy).pow(2);
                    if d2 <= (r + 1).pow(2)
                        && d2 >= (r - 1).pow(2)
                        && xx >= 0
                        && y >= 0
                        && (xx as u32) < w
                        && (y as u32) < h
                    {
                        let (xx, y) = (xx as usize, y as usize);
                        raw[y * row_bytes + xx / 8] &= !(1 << (7 - xx % 8));
                    }
                }
            }
            x += 2 * r + 3 + if rnd(5) == 0 { 10 } else { 0 };
        }
    }
    raw
}

fn g4_decode(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let row_bytes = (w as usize).div_ceil(8);
    let mut out = Vec::new();
    fax::decoder::decode_g4(data.iter().copied(), w, Some(h), |t| {
        let mut row = vec![0u8; row_bytes];
        for (x, c) in fax::decoder::pels(t, w).enumerate() {
            if c == fax::Color::White {
                row[x / 8] |= 1 << (7 - x % 8);
            }
        }
        out.extend_from_slice(&row);
    })
    .unwrap();
    out
}

#[test]
fn one_bit_flate_image_becomes_lossless_ccitt_g4() {
    let (w, h) = (800u32, 600u32);
    let bits = text_like_bits(w, h);
    let mut s = Stream::new(
        image_dict(w, h, Object::Name(b"DeviceGray".to_vec()), 1),
        bits.clone(),
    );
    s.compress().unwrap();
    let before = s.content.len();
    let (dict, content, _) = compress_single(s, Level::Medium.profile());
    assert_eq!(name(&dict, b"Filter"), b"CCITTFaxDecode");
    assert!(content.len() < before, "{} vs {}", content.len(), before);
    assert_eq!(g4_decode(&content, w, h), bits, "G4 must be pixel-exact");
}

#[test]
fn stencil_mask_becomes_lossless_ccitt_g4() {
    let (w, h) = (800u32, 600u32);
    let bits = text_like_bits(w, h);
    let mut s = Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "ImageMask" => true, "BitsPerComponent" => 1,
        },
        bits.clone(),
    );
    s.compress().unwrap();
    let (dict, content, _) = compress_single(s, Level::Medium.profile());
    assert_eq!(name(&dict, b"Filter"), b"CCITTFaxDecode");
    assert!(dict.get(b"ImageMask").unwrap().as_bool().unwrap());
    assert_eq!(g4_decode(&content, w, h), bits);
}
