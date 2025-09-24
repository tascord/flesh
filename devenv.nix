{
  pkgs,
  lib,
  config,
  inputs,
  ...
}:

{
  env.GREET = "FLESH";
  packages = [
    pkgs.lld
    pkgs.libudev-zero
  ];

  languages.rust = {
    enable = true;
    channel = "nightly";
  };

  enterShell = ''
    git status
  '';
}
