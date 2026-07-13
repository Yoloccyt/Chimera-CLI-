class ChimelaCli < Formula
  desc "chimela CLI (NEXUS-OMEGA) terminal interface"
  homepage "https://github.com/Yoloccyt/Chimera-CLI-"
  version "1.5.8-omega"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v1.5.8-omega/chimela-macos-aarch64"
    sha256 "PLACEHOLDER_SHA256_MACOS_AARCH64"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v1.5.8-omega/chimela-macos-x86_64"
    sha256 "PLACEHOLDER_SHA256_MACOS_X86_64"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v1.5.8-omega/chimela-linux-x86_64"
    sha256 "PLACEHOLDER_SHA256_LINUX_X86_64"
  else
    odie "Unsupported platform"
  end

  def install
    if OS.mac? && Hardware::CPU.arm?
      bin.install "chimela-macos-aarch64" => "chimela"
    elsif OS.mac? && Hardware::CPU.intel?
      bin.install "chimela-macos-x86_64" => "chimela"
    elsif OS.linux? && Hardware::CPU.intel?
      bin.install "chimela-linux-x86_64" => "chimela"
    end
  end

  test do
    assert_match(/^(aether|chimera|chimela) \d+\.\d+\.\d+/, shell_output("#{bin}/chimela --version"))
  end
end
