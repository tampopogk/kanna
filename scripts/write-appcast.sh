#!/usr/bin/env bash
set -euo pipefail

usage() { echo "usage: scripts/write-appcast.sh --title T --version V --build N --minimum X.Y.Z --protocol P --url U --length BYTES --signature S" >&2; exit 2; }
while [[ $# -gt 0 ]]; do
  [[ $# -ge 2 ]] || usage
  case "$1" in
    --title) title="$2" ;;
    --version) version="$2" ;;
    --build) build="$2" ;;
    --minimum) minimum="$2" ;;
    --protocol) protocol="$2" ;;
    --url) url="$2" ;;
    --length) length="$2" ;;
    --signature) signature="$2" ;;
    *) usage ;;
  esac
  shift 2
done
for field in title version build minimum protocol url length signature; do [[ -n "${!field:-}" ]] || usage; done

cat <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:kanna="https://kanna.build/appcast">
  <channel>
    <title>$title</title>
    <item>
      <title>$title $version</title>
      <pubDate>$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
      <sparkle:version>$build</sparkle:version>
      <sparkle:shortVersionString>$version</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>$minimum</sparkle:minimumSystemVersion>
      <kanna:protocol>$protocol</kanna:protocol>
      <enclosure url="$url" length="$length" type="application/octet-stream" sparkle:edSignature="$signature"/>
    </item>
  </channel>
</rss>
XML
