{
  lib,
  rustPlatform,
  pkg-config,
  openssl,
  git,
  gh,
  makeWrapper,
}:
let
  manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = manifest.package.name;
  version = manifest.package.version;

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
      ./tests
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    makeWrapper
  ];
  buildInputs = [ openssl ];
  nativeCheckInputs = [ git ];

  preCheck = ''
    export GIT_CONFIG_GLOBAL=/dev/null
    export GIT_CONFIG_NOSYSTEM=1
  '';

  postInstall = ''
    wrapProgram "$out/bin/sk" \
      --prefix PATH : ${
        lib.makeBinPath [
          git
          gh
        ]
      }
  '';

  meta = {
    description = "Local-first stacked branches and GitHub pull requests";
    mainProgram = "sk";
    platforms = lib.platforms.unix;
  };
}
