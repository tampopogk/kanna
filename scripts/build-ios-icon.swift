import AppKit

let args = CommandLine.arguments
let source = URL(fileURLWithPath: args[1])
let output = URL(fileURLWithPath: args[2])
let size = 1024

for variant in ["light", "dark", "tinted"] {
    let svg = source.appendingPathComponent("\(variant).svg")
    guard let image = NSImage(contentsOf: svg) else { fatalError("cannot read \(svg.path)") }
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let info = CGImageAlphaInfo.noneSkipLast.rawValue
    let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space, bitmapInfo: info)!
    NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
    image.draw(in: NSRect(x: 0, y: 0, width: size, height: size))
    let rep = NSBitmapImageRep(cgImage: context.makeImage()!)
    let png = rep.representation(using: .png, properties: [:])!
    try png.write(to: output.appendingPathComponent("\(variant).png"))
}
