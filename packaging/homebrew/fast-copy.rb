# Homebrew cask template for a personal tap (github.com/<owner>/homebrew-tap, file Casks/fast-copy.rb).
# {{VERSION}}, {{DMG_SHA256}}, {{DMG_NAME}} and {{REPO}} are filled by .github/workflows/release.yml
# (rendered copy attached to the draft release in fast-copy-packaging-manifests-<version>.tar.gz).
#   brew tap <owner>/tap && brew install --cask fast-copy
cask "fast-copy" do
  version "{{VERSION}}"
  sha256 "{{DMG_SHA256}}"

  url "https://github.com/{{REPO}}/releases/download/v#{version}/{{DMG_NAME}}"
  name "Fast Copy"
  desc "Fast, verified, resumable file copies"
  homepage "https://github.com/{{REPO}}"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :monterey"

  app "Fast Copy.app"

  # Unsigned builds: uncomment to drop the quarantine flag (or sign + notarize in CI instead).
  # postflight do
  #   system_command "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "#{appdir}/Fast Copy.app"]
  # end

  zap trash: [
    "~/Library/Application Support/mg.fastcopy.desktop",
    "~/Library/Application Support/FastCopy",
    "~/Library/Caches/mg.fastcopy.desktop",
    "~/Library/Preferences/mg.fastcopy.desktop.plist",
    "~/Library/Saved Application State/mg.fastcopy.desktop.savedState",
    "~/Library/WebKit/mg.fastcopy.desktop",
  ]
end
