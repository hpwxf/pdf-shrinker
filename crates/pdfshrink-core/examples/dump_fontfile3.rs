//! Diagnostic: writes every `/FontFile3` program of a PDF to `<dir>/<FontName>-<object number>.cff`.
use lopdf::{Document, Object};

fn main() {
    let mut args = std::env::args().skip(1);
    let (path, dir) = (args.next().unwrap(), args.next().unwrap());
    let doc = Document::load(&path).unwrap();
    for obj in doc.objects.values() {
        let Ok(d) = obj.as_dict() else { continue };
        let Ok(ff) = d.get(b"FontFile3").and_then(Object::as_reference) else {
            continue;
        };
        let name = d
            .get(b"FontName")
            .and_then(Object::as_name)
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        let Ok(s) = doc.get_object(ff).and_then(Object::as_stream) else {
            continue;
        };
        let bytes = s
            .decompressed_content()
            .unwrap_or_else(|_| s.content.clone());
        let sub = s
            .dict
            .get(b"Subtype")
            .and_then(Object::as_name)
            .unwrap_or(b"?");
        println!(
            "{name}: {} bytes, /Subtype {}",
            bytes.len(),
            String::from_utf8_lossy(sub)
        );
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(format!("{dir}/{name}-{}.cff", ff.0), bytes).unwrap();
    }
}
