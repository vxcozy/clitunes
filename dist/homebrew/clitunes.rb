# Homebrew formula for clitunes.
# Update VERSION and SHA256 hashes when cutting a new release.

class Clitunes < Formula
  desc "Terminal music player with internet radio and real-time visualisers"
  homepage "https://github.com/vxcozy/clitunes"
  version "1.0.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/vxcozy/clitunes/releases/download/v#{version}/clitunes-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER"
    else
      url "https://github.com/vxcozy/clitunes/releases/download/v#{version}/clitunes-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/vxcozy/clitunes/releases/download/v#{version}/clitunes-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER"
    else
      url "https://github.com/vxcozy/clitunes/releases/download/v#{version}/clitunes-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  def install
    bin.install "clitunes"
    bin.install "clitunesd"
  end

  test do
    assert_match "clitunes", shell_output("#{bin}/clitunes --help")
  end
end
