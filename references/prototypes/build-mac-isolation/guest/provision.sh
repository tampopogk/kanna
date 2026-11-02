#!/bin/sh
set -eu
pw_file=$1
here=$(cd "$(dirname "$0")" && pwd)
job_pw=$(cat "$pw_file")

if ! id kanna-build >/dev/null 2>&1; then
    sysadminctl -addUser kanna-build -fullName "Kanna Build" -password "$job_pw" -home /Users/kanna-build
    createhomedir -c -u kanna-build >/dev/null
fi
dseditgroup -o edit -a kanna-build -t user _developer
if dseditgroup -o checkmember -m kanna-build admin >/dev/null 2>&1; then
    echo "kanna-build must not be an admin" >&2
    exit 1
fi

xcode-select -s /Applications/Xcode.app/Contents/Developer
xcodebuild -license accept
xcodebuild -runFirstLaunch
DevToolsSecurity -enable
automationmodetool enable-automationmode-without-authentication

mkdir -p /usr/local/kanna
install -m 755 "$here/vsockrelay" /usr/local/kanna/vsockrelay
install -m 644 "$here/agent.py" "$here/wire.py" "$here/model.py" /usr/local/kanna/
for name in dev.kanna.isolation.agent dev.kanna.isolation.relay dev.kanna.isolation.egress; do
    install -m 644 -o root -g wheel "$here/$name.plist" /Library/LaunchAgents/
done

mdutil -a -i off >/dev/null || true
pmset -a sleep 0 displaysleep 0 disksleep 0
defaults write /Library/Preferences/com.apple.loginwindow DisableScreenLock -bool true || true
sudo -u kanna-build defaults -currentHost write com.apple.screensaver idleTime 0
sudo -u kanna-build defaults write com.apple.screensaver askForPassword -int 0 || true

sysadminctl -autologin set -userName kanna-build -password "$job_pw"
defaults read /Library/Preferences/com.apple.loginwindow autoLoginUser
rm -f "$pw_file"
