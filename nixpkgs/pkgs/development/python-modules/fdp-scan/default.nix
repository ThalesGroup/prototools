{
  lib,
  buildPythonPackage,
  fetchFromGitHub,
  rustPlatform,
  protobuf,
  pytestCheckHook,

  # passthru
  nix-update-script,
}:

buildPythonPackage (finalAttrs: {
  pname = "fdp-scan";
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

  buildAndTestSubdir = "fdp-scan-pyo3";

  nativeBuildInputs = [ rustPlatform.cargoSetupHook ];

  build-system = [ rustPlatform.maturinBuildHook ];

  nativeCheckInputs = [
    protobuf
    pytestCheckHook
  ];

  enabledTestPaths = [ "fdp-scan-pyo3/tests" ];

  pythonImportsCheck = [ "fdp_scan_lib" ];

  # Only v-prefixed release tags
  passthru.updateScript = nix-update-script {
    extraArgs = [
      "--version-regex"
      "^v(\\d+\\.\\d+\\.\\d+)$"
    ];
  };

  meta = {
    description = "Python bindings to the scan for protobuf descriptors embedded in binaries";
    homepage = "https://github.com/douzebis/prototools";
    changelog = "https://github.com/douzebis/prototools/blob/${finalAttrs.src.rev}/CHANGELOG.md";
    license = lib.licenses.mit;
    maintainers = with lib.maintainers; [ douzebis ];
  };
})
