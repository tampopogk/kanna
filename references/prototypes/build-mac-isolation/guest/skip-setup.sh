#!/bin/sh
set -eu
v=$(sw_vers -productVersion)
b=$(sw_vers -buildVersion)
d=com.apple.SetupAssistant
for k in DidSeeAccessibility DidSeeActivationLock DidSeeAppStore DidSeeAppearanceSetup DidSeeApplePaySetup \
    DidSeeAvatarSetup DidSeeCloudSetup DidSeeiCloudLoginForStorageServices DidSeeIntelligence DidSeeLockdownMode \
    DidSeePrivacy DidSeeScreenTime DidSeeSiriSetup DidSeeSyncSetup DidSeeSyncSetup2 DidSeeTermsOfAddress \
    DidSeeTouchIDSetup DidSeeTrueTonePrivacy DidSeeUpdateCompleted DidSeeWalletSetup DidSeeFileVault \
    DidSeeLanguageSetup DidSeeKeyboardSetup SkipFirstLoginOptimization; do
    defaults write $d "$k" -bool true
done
for k in LastSeenCloudProductVersion LastSeenSiriProductVersion LastSeenDiagnosticsProductVersion \
    LastSeenIntelligenceProductVersion LastSeenScreenTimeProductVersion LastSeenPrivacyProductVersion; do
    defaults write $d "$k" "$v"
done
defaults write $d LastSeenBuddyBuildVersion "$b"
defaults write $d LastPreLoginTasksPerformedVersion "$v"
defaults write $d LastPreLoginTasksPerformedBuild "$b"
defaults write com.apple.SoftwareUpdate AutomaticallyInstallMacOSUpdates -bool false
defaults read $d | wc -l
