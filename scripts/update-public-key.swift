import CryptoKit
import Foundation

guard CommandLine.arguments.count == 2,
      let text = try? String(contentsOfFile: CommandLine.arguments[1], encoding: .utf8),
      let seed = Data(base64Encoded: text.trimmingCharacters(in: .whitespacesAndNewlines)),
      let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: seed)
else {
    FileHandle.standardError.write(Data("can't read a 32-byte EdDSA seed from that file\n".utf8))
    exit(1)
}
print(key.publicKey.rawRepresentation.base64EncodedString())
