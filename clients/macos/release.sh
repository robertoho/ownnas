#!/usr/bin/env bash
# Builds the Mac app for release and makes build/dist/OwnNAS-<version>-mac.dmg: universal,
# signed with the team's Developer ID, notarized and stapled (the app and the disk image), so
# it opens on any Mac without warnings.
#
# Notarizing needs the team's credentials in this Mac's keychain, stored once with:
#   xcrun notarytool store-credentials ownnas-notary --apple-id <Apple ID> --team-id K27HXR2Q62
# (with an app-specific password from account.apple.com). PROFILE picks another stored name.
set -euo pipefail
cd "$(dirname "$0")"
profile=${PROFILE:-ownnas-notary}
version=$(sed -n 's/^    MARKETING_VERSION: "\(.*\)"$/\1/p' project.yml)
app=build/release/Build/Products/Release/OwnNAS.app
dist=build/dist
dmg=$dist/OwnNAS-$version-mac.dmg
# By its fingerprint: the certificate's name has letters that don't survive being typed.
identity=$(security find-identity -v -p codesigning | awk '/Developer ID Application/ && /K27HXR2Q62/ { print $2; exit }')
[ -n "$identity" ] || { echo "No Developer ID Application certificate for team K27HXR2Q62 in the keychain."; exit 1; }

notarize() {
    local result
    result=$(xcrun notarytool submit "$1" --keychain-profile "$profile" --wait)
    echo "$result" | tail -2
    if ! echo "$result" | grep -q "status: Accepted"; then
        local id
        id=$(echo "$result" | awk '/^  id:/ { print $2; exit }')
        xcrun notarytool log "$id" --keychain-profile "$profile"
        exit 1
    fi
}

disk_image() {
    rm -rf "$dist" && mkdir -p "$dist/stage"
    ditto "$app" "$dist/stage/OwnNAS.app"
    ln -s /Applications "$dist/stage/Applications"
    hdiutil create -quiet -volname OwnNAS -srcfolder "$dist/stage" -ov -format UDZO "$dmg"
    rm -rf "$dist/stage"
    codesign --sign "$identity" --timestamp "$dmg"
}

echo "Building OwnNAS $version for Mac…"
xcodegen -q
xcodebuild -quiet -project OwnNAS.xcodeproj -scheme OwnNAS -configuration Release -derivedDataPath build/release \
    -destination 'generic/platform=macOS' OTHER_CODE_SIGN_FLAGS=--timestamp build
codesign --verify --deep --strict "$app"

# Notarized once to staple the app itself (so it opens offline once copied out of the image),
# then again as the image that holds the stapled app.
echo "Notarizing the app…"
disk_image
notarize "$dmg"
xcrun stapler staple -q "$app"
echo "Notarizing the disk image…"
disk_image
notarize "$dmg"
xcrun stapler staple -q "$dmg"
spctl -a -t open --context context:primary-signature "$dmg"
shasum -a 256 "$dmg"
echo "Ready: clients/macos/$dmg"
