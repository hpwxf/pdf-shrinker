#!/usr/bin/env bash
# Generates annotated PDFs for testing annotation flattening (`--flatten`,
# D-027): every annotation kind macOS PDFKit (the framework behind Preview)
# writes, with the appearance streams and embedded TrueType subsets it writes
# for them — markup (FreeText, Ink, Stamp, Square, Circle, Line, Highlight,
# Underline, StrikeOut), what must survive (sticky note + popup, link) and
# form fields (text, checkbox, signature).
# Nothing is committed: rerun this whenever needed.
#
#   scripts/make-annotated-pdfs.sh [OUT_DIR]      (default: ./test-pdfs)
#
# macOS only: needs `swift` (Xcode command-line tools). Writes:
#   annotated-all.pdf        one page, every kind above
#   annotated-rotated.pdf    /Rotate 90 page, one annotation flagged NoRotate
#   annotated-multipage.pdf  3 pages, annotations on pages 1 and 3 only
#   annotated-form.pdf       form fields only (filled text, checked box, signature)
set -euo pipefail

OUT="${1:-test-pdfs}"
command -v swift >/dev/null || { echo "missing tool: swift (xcode-select --install)" >&2; exit 1; }
mkdir -p "$OUT"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/annotate.swift" <<'SWIFT'
import AppKit
import CoreText
import PDFKit

let out = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let a4 = CGRect(x: 0, y: 0, width: 595.28, height: 841.89)

/// A plain document: `pages` pages of real (embedded-font) text, as a scanner
/// or a word processor would leave it before anyone annotates it.
func basePDF(pages: Int) -> PDFDocument {
    let data = NSMutableData()
    var box = a4
    let ctx = CGContext(consumer: CGDataConsumer(data: data)!, mediaBox: &box, nil)!
    let font = CTFontCreateWithName("Times-Roman" as CFString, 12, nil)
    let bold = CTFontCreateWithName("Helvetica-Bold" as CFString, 16, nil)
    for p in 1...pages {
        ctx.beginPDFPage(nil)
        func line(_ s: String, _ y: CGFloat, _ f: CTFont = font) {
            let attr = NSAttributedString(string: s, attributes: [.font: f])
            ctx.textPosition = CGPoint(x: 56, y: y)
            CTLineDraw(CTLineCreateWithAttributedString(attr), ctx)
        }
        line("Déclaration de sinistre — page \(p)/\(pages)", 780, bold)
        let labels = ["Nom du déclarant :", "Adresse du lieu du sinistre :", "Assureur :",
                      "Téléphone :", "Date du sinistre :", "Description des dommages :"]
        for (i, l) in labels.enumerated() {
            line(l, 740 - CGFloat(i) * 28)
            ctx.setLineWidth(0.5)
            ctx.move(to: CGPoint(x: 230, y: 737 - CGFloat(i) * 28))
            ctx.addLine(to: CGPoint(x: 540, y: 737 - CGFloat(i) * 28))
            ctx.strokePath()
        }
        for i in 0..<12 {
            line("Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor \(i).",
                 520 - CGFloat(i) * 16)
        }
        ctx.endPDFPage()
    }
    ctx.closePDF()
    return PDFDocument(data: data as Data)!
}

func freeText(_ page: PDFPage, _ s: String, _ r: CGRect) -> PDFAnnotation {
    let a = PDFAnnotation(bounds: r, forType: .freeText, withProperties: nil)
    a.contents = s
    a.font = NSFont(name: "Helvetica", size: 11)
    a.fontColor = .black
    a.color = .clear
    a.border = PDFBorder()
    a.border?.lineWidth = 0
    page.addAnnotation(a)
    return a
}

func markup(_ page: PDFPage, _ type: PDFAnnotationSubtype, _ r: CGRect, _ color: NSColor) {
    let a = PDFAnnotation(bounds: r, forType: type, withProperties: nil)
    a.color = color
    a.quadrilateralPoints = [
        NSValue(point: NSPoint(x: 0, y: r.height)), NSValue(point: NSPoint(x: r.width, y: r.height)),
        NSValue(point: NSPoint(x: 0, y: 0)), NSValue(point: NSPoint(x: r.width, y: 0)),
    ]
    page.addAnnotation(a)
}

/// A stamp drawn by our own code, as Preview stores a signature: PDFKit writes
/// whatever `draw` paints as its appearance stream — here a raster image with
/// an alpha channel, so an image XObject with a soft mask.
final class SignatureStamp: PDFAnnotation {
    lazy var image: CGImage = {
        let (w, h) = (420, 120)
        let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                            space: CGColorSpaceCreateDeviceRGB(),
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        ctx.setStrokeColor(NSColor(calibratedRed: 0.05, green: 0.1, blue: 0.45, alpha: 1).cgColor)
        ctx.setLineWidth(5)
        ctx.setLineCap(.round)
        ctx.move(to: CGPoint(x: 15, y: 50))
        ctx.addCurve(to: CGPoint(x: 140, y: 70), control1: CGPoint(x: 40, y: 140), control2: CGPoint(x: 90, y: -20))
        ctx.addCurve(to: CGPoint(x: 280, y: 55), control1: CGPoint(x: 190, y: 130), control2: CGPoint(x: 220, y: 0))
        ctx.addCurve(to: CGPoint(x: 405, y: 75), control1: CGPoint(x: 320, y: 100), control2: CGPoint(x: 360, y: 30))
        ctx.strokePath()
        return ctx.makeImage()!
    }()

    override func draw(with box: PDFDisplayBox, in context: CGContext) {
        context.draw(image, in: bounds)
    }
}

func everyKind(_ page: PDFPage) {
    // Lines of the form: labels at y = 740, 712, 684, 656, 628, 600.
    _ = freeText(page, "Pascal Dupont", CGRect(x: 232, y: 738, width: 200, height: 15))
    _ = freeText(page, "46 rue des Lilas, 76000 Rouen", CGRect(x: 232, y: 710, width: 250, height: 15))
    _ = freeText(page, "Assurances Exemple", CGRect(x: 232, y: 682, width: 250, height: 15))
    // Multi-line text box (description).
    _ = freeText(page, "Dégât des eaux : plafond de la salle de bains\net mur de la cuisine adjacente",
                 CGRect(x: 232, y: 562, width: 300, height: 34))

    markup(page, .highlight, CGRect(x: 54, y: 516, width: 300, height: 14), .yellow)
    markup(page, .underline, CGRect(x: 54, y: 500, width: 300, height: 14), .blue)
    markup(page, .strikeOut, CGRect(x: 54, y: 484, width: 300, height: 14), .red)

    // Below the text: shapes, a line with an arrow, a freehand drawing.
    for (type, r, c) in [(PDFAnnotationSubtype.square, CGRect(x: 54, y: 240, width: 120, height: 60), NSColor.red),
                         (.circle, CGRect(x: 200, y: 240, width: 80, height: 60), NSColor.green)] {
        let a = PDFAnnotation(bounds: r, forType: type, withProperties: nil)
        a.color = c
        a.border = PDFBorder()
        a.border?.lineWidth = 2
        page.addAnnotation(a)
    }
    let line = PDFAnnotation(bounds: CGRect(x: 310, y: 240, width: 220, height: 60), forType: .line, withProperties: nil)
    line.startPoint = NSPoint(x: 0, y: 0)
    line.endPoint = NSPoint(x: 220, y: 60)
    line.endLineStyle = .openArrow
    line.color = .purple
    page.addAnnotation(line)

    let ink = PDFAnnotation(bounds: CGRect(x: 54, y: 150, width: 200, height: 60), forType: .ink, withProperties: nil)
    let path = NSBezierPath()
    path.move(to: NSPoint(x: 5, y: 30))
    path.curve(to: NSPoint(x: 195, y: 30), controlPoint1: NSPoint(x: 60, y: 70), controlPoint2: NSPoint(x: 130, y: -10))
    ink.add(path)
    ink.color = .blue
    ink.border = PDFBorder()
    ink.border?.lineWidth = 2
    page.addAnnotation(ink)

    // A stock stamp, and a signature the way Preview stores it (image stamp).
    let stamp = PDFAnnotation(bounds: CGRect(x: 54, y: 60, width: 140, height: 50), forType: .stamp, withProperties: nil)
    stamp.stampName = "Approved"
    page.addAnnotation(stamp)
    page.addAnnotation(SignatureStamp(bounds: CGRect(x: 380, y: 60, width: 168, height: 48), forType: .stamp, withProperties: nil))

    // Must survive flattening: a sticky note (PDFKit drops its popup), a link.
    let note = PDFAnnotation(bounds: CGRect(x: 545, y: 740, width: 20, height: 20), forType: .text, withProperties: nil)
    note.contents = "Vérifier le numéro de contrat"
    note.iconType = .comment
    page.addAnnotation(note)
    let link = PDFAnnotation(bounds: CGRect(x: 54, y: 437, width: 300, height: 14), forType: .link, withProperties: nil)
    link.url = URL(string: "https://example.org/")
    link.border = PDFBorder()
    link.border?.lineWidth = 0
    page.addAnnotation(link)
}

func formFields(_ page: PDFPage) {
    let text = PDFAnnotation(bounds: CGRect(x: 232, y: 654, width: 200, height: 16), forType: .widget, withProperties: nil)
    text.widgetFieldType = .text
    text.fieldName = "telephone"
    text.font = NSFont(name: "Helvetica", size: 11)
    text.widgetStringValue = "02 32 80 91 91"
    page.addAnnotation(text)

    let check = PDFAnnotation(bounds: CGRect(x: 232, y: 626, width: 14, height: 14), forType: .widget, withProperties: nil)
    check.widgetFieldType = .button
    check.widgetControlType = .checkBoxControl
    check.fieldName = "urgent"
    check.buttonWidgetState = .onState
    page.addAnnotation(check)

    // Empty signature field (what Preview leaves in A.pdf: zero-size appearance).
    let sig = PDFAnnotation(bounds: CGRect(x: 380, y: 120, width: 168, height: 40), forType: .widget, withProperties: nil)
    sig.widgetFieldType = .signature
    sig.fieldName = "Signature"
    page.addAnnotation(sig)
}

func write(_ doc: PDFDocument, _ name: String) {
    let url = out.appendingPathComponent(name)
    guard doc.write(to: url) else { fatalError("could not write \(url.path)") }
    print("wrote \(url.path)")
}

do {
    let doc = basePDF(pages: 1)
    everyKind(doc.page(at: 0)!)
    formFields(doc.page(at: 0)!)
    write(doc, "annotated-all.pdf")
}
do {
    let doc = basePDF(pages: 1)
    let page = doc.page(at: 0)!
    page.rotation = 90
    // Bounds in unrotated page space. PDFKit counter-rotates its FreeText
    // appearance so it reads upright on screen; NoRotate turns it once more.
    _ = freeText(page, "Page tournée : sans drapeau", CGRect(x: 60, y: 560, width: 16, height: 200))
    let upright = freeText(page, "Page tournée : NoRotate", CGRect(x: 90, y: 560, width: 16, height: 200))
    upright.setValue(4 | 16, forAnnotationKey: .flags) // Print | NoRotate
    write(doc, "annotated-rotated.pdf")
}
do {
    let doc = basePDF(pages: 3)
    _ = freeText(doc.page(at: 0)!, "Page 1 annotée", CGRect(x: 232, y: 738, width: 200, height: 15))
    _ = freeText(doc.page(at: 2)!, "Page 3 annotée", CGRect(x: 232, y: 738, width: 200, height: 15))
    markup(doc.page(at: 2)!, .highlight, CGRect(x: 54, y: 516, width: 300, height: 14), .yellow)
    write(doc, "annotated-multipage.pdf")
}
do {
    let doc = basePDF(pages: 1)
    formFields(doc.page(at: 0)!)
    write(doc, "annotated-form.pdf")
}
SWIFT

swift "$TMP/annotate.swift" "$OUT"
