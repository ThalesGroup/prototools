{
  lib,
  buildPythonPackage,
  fetchFromGitHub,
  rustPlatform,
}:

buildPythonPackage (finalAttrs: {
  pname = "prototext-graph";
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

  buildAndTestSubdir = "prototext-graph-pyo3";

  nativeBuildInputs = [ rustPlatform.cargoSetupHook ];

  build-system = [ rustPlatform.maturinBuildHook ];

  # There are no Python tests
  doCheck = false;

  pythonImportsCheck = [ "prototext_graph_lib" ];

  meta = {
    description = "Python bindings to the prototools scoring graph builder";
    homepage = "https://github.com/douzebis/prototools";
    changelog = "https://github.com/douzebis/prototools/blob/${finalAttrs.src.rev}/CHANGELOG.md";
    license = lib.licenses.mit;
    maintainers = with lib.maintainers; [ douzebis ];
  };
})
