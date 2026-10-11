{
  lib,
  buildPythonPackage,
  fetchFromGitHub,
  rustPlatform,
  protobuf,
  pytestCheckHook,
}:

buildPythonPackage (finalAttrs: {
  pname = "fdp-scan";
  version = "0.3.2";
  pyproject = true;

  src = fetchFromGitHub {
    owner = "douzebis";
    repo = "prototools";
    tag = "v${finalAttrs.version}";
    hash = "sha256-wNeO5GUz0XkUpdiIGp4xWXf7Ol2pmfnqspCbeTAKmKs=";
  };

  cargoDeps = rustPlatform.fetchCargoVendor {
    inherit (finalAttrs) pname version src;
    hash = "sha256-4F/ZRjvaFPTJALcRZ0j8bMwIRAaTTghf4fCbx2+6z3M=";
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

  meta = {
    description = "Python bindings to the scan for protobuf descriptors embedded in binaries";
    homepage = "https://github.com/douzebis/prototools";
    changelog = "https://github.com/douzebis/prototools/blob/${finalAttrs.src.rev}/CHANGELOG.md";
    license = lib.licenses.mit;
    maintainers = with lib.maintainers; [ douzebis ];
  };
})
