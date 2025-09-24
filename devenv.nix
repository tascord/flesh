{
  pkgs,
  lib,
  config,
  inputs,
  ...
}:

{
  env.GREET = "FLESH";
  packages = [ pkgs.lld ];

  languages.rust = {
    enable = true;
    channel = "nightly";
  };

  enterShell = ''
    git status
  '';
}
