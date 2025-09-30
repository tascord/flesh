{
  rustPlatform,
  rustfmt,
}:

rustPlatform.buildRustPackage {
  pname = "flesh";
  version = "alpha";

  src = ./crates/flesh;
  cargoLock = {
    lockFile = ./crates/flesh/Cargo.lock;
  };
  postInstall = ''
    cp -r $src/statuses.csv $out

  '';
  nativeBuildInputs = [ rustfmt ];

}
