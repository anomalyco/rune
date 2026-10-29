// CoreText references for `rune::text::pixel_compare`.
// usage: swift scripts/coretext-reference.swift cases-2x.json OUT_DIR
//
// Draws each case's glyphs (Rune's glyph ids at its unquantised pen positions and whole-pixel
// baselines) with CTFontDrawGlyphs into an opaque sRGB bitmap, one sheet per entry of "sheets":
// antialiasing, subpixel positioning and subpixel quantisation on, as macOS draws window text,
// with font smoothing on or off. Inter comes from its files; "system" cases use CoreText's own
// system font (CTFontCreateUIFontForLanguage) at the case's wght and opsz. Sheets keep the green
// channel. No window or screen access.
import CoreGraphics
import CoreText
import Foundation
import ImageIO
import UniformTypeIdentifiers

let args = CommandLine.arguments
let spec = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: args[1]))) as! [String: Any]
let out = URL(fileURLWithPath: args[2])
let scale = CGFloat(spec["scale"] as! Double)
let width = spec["width"] as! Int, height = spec["height"] as! Int
let paths = spec["fonts"] as! [String: Any]

func fileFont(_ path: String, _ size: CGFloat) -> CTFont {
    let url = URL(fileURLWithPath: path) as CFURL
    let desc = (CTFontManagerCreateFontDescriptorsFromURL(url) as! [CTFontDescriptor])[0]
    return CTFontCreateWithFontDescriptor(desc, size, nil)
}

func systemFont(_ size: CGFloat, weight: Double, opsz: Double) -> CTFont {
    let base = CTFontCreateUIFontForLanguage(.system, size, nil)!
    // Rune shapes the same file at the same instance; anything else would draw other glyphs.
    let url = CTFontCopyAttribute(base, kCTFontURLAttribute) as! URL
    let rune = paths["system"] as? String ?? ""
    precondition(url.resolvingSymlinksInPath().path == URL(fileURLWithPath: rune).resolvingSymlinksInPath().path,
        "CoreText's system font is \(url.path), Rune loaded \(rune)")
    let axes = [NSNumber(value: 0x7767_6874): weight, NSNumber(value: 0x6f70_737a): opsz] as [NSNumber: Double]
    let desc = CTFontDescriptorCreateWithAttributes([kCTFontVariationAttribute: axes] as CFDictionary)
    return CTFontCreateCopyWithAttributes(base, size, nil, desc)
}

func color(_ hex: String) -> CGColor {
    let v = Int(hex.dropFirst(), radix: 16)!
    return CGColor(srgbRed: CGFloat((v >> 16) & 255) / 255, green: CGFloat((v >> 8) & 255) / 255, blue: CGFloat(v & 255) / 255, alpha: 1)
}

let cases = spec["cases"] as! [[String: Any]]
let fonts: [CTFont] = cases.map { c in
    let size = CGFloat(c["size"] as! Double)
    switch c["font"] as! String {
    case "system": return systemFont(size, weight: c["weight"] as! Double, opsz: c["opsz"] as! Double)
    case let name: return fileFont(paths[name] as! String, size)
    }
}

for sheet in spec["sheets"] as! [[String: Any]] {
    let smooth = sheet["smooth"] as! Bool
    let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
        space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.setFillColor(color(sheet["bg"] as! String))
    ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))
    // Top-left origin in points.
    ctx.translateBy(x: 0, y: CGFloat(height))
    ctx.scaleBy(x: scale, y: -scale)
    ctx.setAllowsAntialiasing(true); ctx.setShouldAntialias(true)
    ctx.setAllowsFontSmoothing(smooth); ctx.setShouldSmoothFonts(smooth)
    ctx.setAllowsFontSubpixelPositioning(true); ctx.setShouldSubpixelPositionFonts(true)
    ctx.setAllowsFontSubpixelQuantization(true); ctx.setShouldSubpixelQuantizeFonts(true)
    ctx.setFillColor(color(sheet["fg"] as! String))
    // The flipped text matrix draws glyphs upright under the y-down CTM; positions are in text
    // space, so y is negated.
    ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
    for (c, f) in zip(cases, fonts) {
        let rows = c["glyphs"] as! [[Double]]
        var glyphs = rows.map { CGGlyph($0[0]) }
        var positions = rows.map { CGPoint(x: CGFloat($0[1]) / scale, y: -CGFloat($0[2]) / scale) }
        CTFontDrawGlyphs(f, &glyphs, &positions, glyphs.count, ctx)
    }
    // Keep the green channel only, as raw bytes: the comparison measures green, and one
    // channel keeps the sheets small.
    let rgba = ctx.data!.bindMemory(to: UInt8.self, capacity: width * height * 4)
    var green = [UInt8](repeating: 0, count: width * height)
    for i in 0..<(width * height) { green[i] = rgba[i * 4 + 1] }
    let grey = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 8, bytesPerRow: width,
        space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.none.rawValue),
        provider: CGDataProvider(data: Data(green) as CFData)!, decode: nil, shouldInterpolate: false, intent: .defaultIntent)!
    let name = sheet["file"] as! String
    let sink = CGImageDestinationCreateWithURL(out.appendingPathComponent(name) as CFURL, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(sink, grey, nil)
    precondition(CGImageDestinationFinalize(sink))
    print(name)
}
