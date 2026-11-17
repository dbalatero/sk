class Sk < Formula
  desc "Local-first stacked branches and GitHub pull requests"
  homepage "https://github.com/dbalatero/sk"
  head "https://github.com/dbalatero/sk.git", branch: "main"

  depends_on "pkgconf" => :build
  depends_on "rust" => :build
  depends_on "gh"
  depends_on "git"

  on_linux do
    depends_on "openssl@3"
  end

  def install
    system "cargo", "install", *std_cargo_args(root: libexec)
    (bin/"sk").write_env_script libexec/"bin/sk",
      PATH: "#{Formula["git"].opt_bin}:#{Formula["gh"].opt_bin}:$PATH"
  end

  test do
    ENV["XDG_CONFIG_HOME"] = testpath/"config"
    ENV["GIT_CONFIG_GLOBAL"] = "/dev/null"
    ENV["GIT_CONFIG_NOSYSTEM"] = "1"
    ENV["GIT_ALLOW_PROTOCOL"] = ""
    git = Formula["git"].opt_bin/"git"
    system git, "init", "-b", "main"
    system git, "config", "user.name", "Homebrew Test"
    system git, "config", "user.email", "test@example.invalid"
    system git, "-c", "commit.gpgsign=false", "commit", "--allow-empty",
      "-m", "Initial commit"
    system bin/"sk", "create", "feature"
    assert_equal "feature", shell_output("#{git} branch --show-current").strip
    assert_match "feature", shell_output("#{bin}/sk log short")
    assert_match "--insert", shell_output("#{bin}/sk create --help")
  end
end
