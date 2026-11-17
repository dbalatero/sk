{
  description = "Local-first stacked branches and GitHub pull requests";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (system: {
        sk = nixpkgs.legacyPackages.${system}.callPackage ./package.nix { };
        default = self.packages.${system}.sk;
      });

      checks = forAllSystems (system: {
        inherit (self.packages.${system}) sk;
      });

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
