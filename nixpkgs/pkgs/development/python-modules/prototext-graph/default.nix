{
  lib,
  buildPythonPackage,
  fetchFromGitHub,
  rustPlatform,

  # passthru
  nix-update-script,
}:

buildPythonPackage (finalAttrs: {
  pname = "prototext-graph";
  version = "0.4.0";
  pyproject = true;
  __structuredAttrs = true;

  src = fetchFromGitHub {
    owner = "douzebis";
    repo = "prototools";
    tag = "v${finalAttrs.version}";
    hash = "sha256-zE8jDoPISjJg56cRjs0LJDDBQv9dWYAryg29RpZLsZU=";
  };

  cargoDeps = rustPlatform.fetchCargoVendor {
    inherit (finalAttrs) pname version src;
    hash = "sha256-0r9goxXjNTPSdktSC7moIg39okYVFwEpoKwmSfQV23U=";
  };

  buildAndTestSubdir = "prototext-graph-pyo3";

  nativeBuildInputs = [ rustPlatform.cargoSetupHook ];

  build-system = [ rustPlatform.maturinBuildHook ];

  # There are no Python tests
  doCheck = false;

  pythonImportsCheck = [ "prototext_graph_lib" ];

  # Only v-prefixed release tags
  passthru.updateScript = nix-update-script {
    extraArgs = [
      "--version-regex"
      "^v(\\d+\\.\\d+\\.\\d+)$"
    ];
  };

  meta = {
    description = "Python bindings to the prototools scoring graph builder";
    homepage = "https://github.com/douzebis/prototools";
    changelog = "https://github.com/douzebis/prototools/blob/${finalAttrs.src.rev}/CHANGELOG.md";
    license = lib.licenses.mit;
    maintainers = with lib.maintainers; [ douzebis ];
  };
})
