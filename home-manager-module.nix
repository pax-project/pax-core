# home-manager module for pax, exposed by this flake as `homeModules.default`.
# Usage: programs.pax.enable = true;
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.pax;
in
{
  options.programs.pax = {
    enable = lib.mkEnableOption "pax, a tool for discovering, declaring, and reproducibly acquiring academic papers";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "pax.packages.<system>.default";
      description = "The pax package to install.";
    };

    # pax has no config file: the `pax` binary reads these as plain
    # environment variables (optionally via a `.env` in the current
    # directory, loaded by dotenvy) rather than parsing a TOML/JSON/YAML
    # file at a fixed path, so `settings` maps directly to session
    # environment variables instead of an xdg.configFile render.
    settings = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      example = lib.literalExpression ''
        {
          SEMANTIC_SCHOLAR_API_KEY = "...";
          ARXIV_CONTACT = "you@example.com";
          PAX_PDF_VIEWER = "zathura";
        }
      '';
      description = ''
        Environment variables for `pax`. Recognized keys:
        `SEMANTIC_SCHOLAR_API_KEY` (Semantic Scholar falls back to
        unauthenticated requests if unset), `ARXIV_CONTACT` (arXiv's
        contact-email etiquette header, omitted if unset), and
        `PAX_PDF_VIEWER` (defaults to `xdg-open`).
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
    home.sessionVariables = cfg.settings;
  };
}
