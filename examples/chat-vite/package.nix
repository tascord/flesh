{
  nodejs,
  rustPlatform,
  rustfmt,
}:
rustPlatform.buildRustPackage {
  pname = "demo";
  version = "alpha";
  src = ../../.;
  buildAndTestSubdir = "./examples/chat-vite";

  cargoLock = {
    lockFile = ../../Cargo.lock;
  };
  nativeBuildInputs = [
    nodejs
    rustfmt
  ];

}
