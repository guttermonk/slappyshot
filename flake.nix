{
  description = "Screenshot annotation tool (egui/eframe)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    ...
  }: let
    systems = ["x86_64-linux" "aarch64-linux"];
    forEachSystem = nixpkgs.lib.genAttrs systems;
    mkPkgs = system:
      import nixpkgs {
        inherit system;
        overlays = [rust-overlay.overlays.default];
      };

    # Build-time link deps
    nativeLibs = pkgs:
      with pkgs; [
        libGL
        mesa
        wayland
        libxkbcommon
        xorg.libX11
        xorg.libXcursor
        xorg.libXrandr
        xorg.libXi
        xorg.libxcb
      ];

    # Runtime LD_LIBRARY_PATH — uses nixpkgs mesa (software rasterizer) so we
    # never need system GPU drivers and avoid wayland version mismatches.
    runtimeLibs = pkgs:
      with pkgs; [
        libGL
        mesa
        wayland
        libxkbcommon
        xorg.libX11
        xorg.libXcursor
        xorg.libXrandr
        xorg.libXi
        xorg.libxcb
      ];
  in {
    packages = forEachSystem (system: let
      pkgs = mkPkgs system;
      # image 0.25.10 requires rustc >= 1.88.0; nixpkgs ships 1.86.0
      rustToolchain = rust-overlay.packages.${system}.rust;
      rustPlatform = pkgs.makeRustPlatform {
        cargo = rustToolchain;
        rustc = rustToolchain;
      };
    in {
      default = rustPlatform.buildRustPackage {
        pname = "slappyshot";
        version = "0.21.0";
        src = self;

        cargoLock.lockFile = ./Cargo.lock;

        nativeBuildInputs = with pkgs; [
          pkg-config
          makeWrapper
        ];

        buildInputs = nativeLibs pkgs;

        # Use nixpkgs mesa (llvmpipe software rasterizer) — self-contained,
        # no system GPU driver dependency, no wayland version mismatches.
        postInstall = ''
          wrapProgram $out/bin/slappyshot \
            --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath (runtimeLibs pkgs)} \
            --set LIBGL_ALWAYS_SOFTWARE 1 \
            --set __EGL_VENDOR_LIBRARY_DIRS ${pkgs.mesa}/share/glvnd/egl_vendor.d \
            --set LIBGL_DRIVERS_PATH ${pkgs.mesa}/lib/dri
        '';
      };
    });

    devShells = forEachSystem (system: let
      pkgs = mkPkgs system;
    in {
      default = pkgs.mkShell {
        buildInputs =
          (nativeLibs pkgs)
          ++ (with pkgs; [
            pkg-config
            gcc # .cargo/config.toml sets linker = "gcc"
            (rust-overlay.packages.${system}.rust.override {
              extensions = ["rust-src" "rust-analyzer"];
            })
          ]);

        shellHook = ''
          export LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath (runtimeLibs pkgs)}:$LD_LIBRARY_PATH
          export LIBGL_ALWAYS_SOFTWARE=1
          export __EGL_VENDOR_LIBRARY_DIRS=${pkgs.mesa}/share/glvnd/egl_vendor.d
          export LIBGL_DRIVERS_PATH=${pkgs.mesa}/lib/dri
        '';
      };
    });
  };
}
