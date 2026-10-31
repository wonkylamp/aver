# Template for Formula/aver.rb in the wonkylamp/homebrew-tap repository, which
# makes `brew install wonkylamp/tap/aver` work. Do not edit the placeholders by
# hand: packaging/homebrew/update-formula.sh <version> fills them from the
# release's SHA256SUMS.
class Aver < Formula
  desc "Version control and verification for AI coding agents"
  homepage "https://github.com/wonkylamp/aver"
  version "@VERSION@"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/wonkylamp/aver/releases/download/v#{version}/aver-darwin-aarch64"
      sha256 "@SHA256_DARWIN_AARCH64@"
    end
    on_intel do
      url "https://github.com/wonkylamp/aver/releases/download/v#{version}/aver-darwin-x86_64"
      sha256 "@SHA256_DARWIN_X86_64@"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/wonkylamp/aver/releases/download/v#{version}/aver-linux-aarch64"
      sha256 "@SHA256_LINUX_AARCH64@"
    end
    on_intel do
      url "https://github.com/wonkylamp/aver/releases/download/v#{version}/aver-linux-x86_64"
      sha256 "@SHA256_LINUX_X86_64@"
    end
  end

  def install
    bin.install Dir["aver-*"].first => "aver"
  end

  def caveats
    <<~EOS
      Try it offline in a throwaway directory with `aver demo`. In your own
      repository, `aver init` sets it up and `aver` prints the next command.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/aver --version")
  end
end
