// No AppKit, window, or screen access. Matched positions isolate rasterization from shaping.
import Foundation
import CoreText
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

let dir = URL(fileURLWithPath: CommandLine.arguments[1])
let nativeOpticalSize = CommandLine.arguments.contains("--native-opsz")
var shaping: [[String: Any]] = []
for url in try FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)
    where url.pathExtension == "json" && (url.lastPathComponent.hasPrefix("light-") || url.lastPathComponent.hasPrefix("dark-")) {
    let data = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
    let width = data["width"] as! Int, height = data["height"] as! Int
    let scale = data["scale"] as! Double
    let fontURL = URL(fileURLWithPath: data["font"] as! String)
    let provider = CGDataProvider(url: fontURL as CFURL)!
    let cgfont = CGFont(provider)!
    let cs = CGColorSpace(name: CGColorSpace.sRGB)!
    let ctx = CGContext(data:nil,width:width,height:height,bitsPerComponent:8,bytesPerRow:width*4,
        space:cs,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
    let fg = CGFloat(data["fg"] as! Int)/255, bg = CGFloat(data["bg"] as! Int)/255
    ctx.setFillColor(red:bg,green:bg,blue:bg,alpha:1)
    ctx.fill(CGRect(x:0,y:0,width:width,height:height))
    ctx.setFillColor(red:fg,green:fg,blue:fg,alpha:1)
    ctx.setAllowsAntialiasing(true); ctx.setShouldAntialias(true)
    ctx.setAllowsFontSmoothing(true); ctx.setShouldSmoothFonts(true)
    ctx.setAllowsFontSubpixelPositioning(true); ctx.setShouldSubpixelPositionFonts(true)
    ctx.setAllowsFontSubpixelQuantization(false); ctx.setShouldSubpixelQuantizeFonts(false)
    // Preserve nominal point size: CoreText optical smoothing can depend on it.
    ctx.scaleBy(x:scale,y:scale)
    for row in data["rows"] as! [[String:Any]] {
        let base = CTFontCreateWithGraphicsFont(cgfont,CGFloat(row["size"] as! Double),nil,nil)
        // Parley's current optical-size selection uses the run's device-pixel size.
        // Pin the reference to that same variation; otherwise SF compares different outlines.
        let opsz = row["opsz"] as? Double ?? (row["size"] as! Double)*scale
        let attrs = [kCTFontVariationAttribute: [NSNumber(value:0x6f70737a): opsz]] as CFDictionary
        let font = nativeOpticalSize ? base : CTFontCreateCopyWithAttributes(base,0,nil,CTFontDescriptorCreateWithAttributes(attrs))
        let attributed = NSAttributedString(string: row["text"] as! String,
            attributes: [NSAttributedString.Key(kCTFontAttributeName as String): font])
        let line = CTLineCreateWithAttributedString(attributed)
        let advance = CTLineGetTypographicBounds(line, nil, nil, nil) * scale
        shaping.append(["sample": url.lastPathComponent, "size": row["size"]!, "text": row["text"]!,
            "rune_advance": row["advance"] ?? 0, "coretext_advance": advance,
            "coretext_variations": String(describing: CTFontCopyVariation(font))])
        let items = row["glyphs"] as! [[String:Any]]
        var glyphs = items.map { CGGlyph($0["id"] as! Int) }
        var positions = items.map { CGPoint(x:($0["x"] as! Double)/scale,y:(Double(height)-($0["y"] as! Double))/scale) }
        CTFontDrawGlyphs(font,&glyphs,&positions,glyphs.count,ctx)
    }
    let dest = dir.appendingPathComponent("coretext-"+url.deletingPathExtension().lastPathComponent+".png")
    let sink = CGImageDestinationCreateWithURL(dest as CFURL,UTType.png.identifier as CFString,1,nil)!
    CGImageDestinationAddImage(sink,ctx.makeImage()!,nil)
    precondition(CGImageDestinationFinalize(sink))
    print(dest.lastPathComponent)
}
try JSONSerialization.data(withJSONObject: shaping, options: [.prettyPrinted, .sortedKeys])
    .write(to: dir.appendingPathComponent("coretext-shaping.json"))
