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
    channel = "stable";
  };

  enterShell = ''
    git status
  '';
}
