//! Throwaway fixture generator for manual smoke-testing the CLI/app; not part
//! of the shipped product. `cargo run -p pdfshrink-core --example make_fixture -- out.pdf`
use std::env;

use image::codecs::jpeg::JpegEncoder;
use image::{ImageBuffer, Rgb};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, Stream, dictionary};

fn main() {
    let out = env::args().nth(1).unwrap_or_else(|| "fixture.pdf".to_string());

    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(1600, 2000, |x, y| {
        Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, (((x + y) / 2) % 256) as u8])
    });
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 95).encode_image(&img).unwrap();

    let img_dict = dictionary! {
        "Type" => "XObject", "Subtype" => "Image",
        "Width" => 1600i64, "Height" => 2000i64,
        "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "DCTDecode",
    };
    let image_stream = Stream::new(img_dict, jpeg).with_compression(false);

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let img_id = doc.add_object(image_stream);

    let ops = vec![
        Operation::new("q", vec![]),
        Operation::new(
            "cm",
            vec![
                Object::Real(400.0),
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(500.0),
                Object::Real(106.0),
                Object::Real(146.0),
            ],
        ),
        Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
        Operation::new("Q", vec![]),
    ];
    let content_bytes = Content { operations: ops }.encode().unwrap();
    let content_id = doc.add_object(Stream::new(Dictionary::new(), content_bytes));

    let resources = dictionary! { "XObject" => dictionary! { "Im0" => Object::Reference(img_id) } };
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(pages_id),
        "Contents" => Object::Reference(content_id),
        "Resources" => resources,
        "MediaBox" => vec![Object::Real(0.0), Object::Real(0.0), Object::Real(612.0), Object::Real(792.0)],
    });
    let pages = dictionary! { "Type" => "Pages", "Count" => 1, "Kids" => vec![Object::Reference(page_id)] };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(pages_id) });
    doc.trailer.set("Root", Object::Reference(catalog_id));

    doc.save(&out).unwrap();
    println!("wrote {out}");
}
