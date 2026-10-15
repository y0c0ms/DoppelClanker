# winget manifest (not submitted)

These templates follow the winget manifest schema v1.10.0
(microsoft/winget-cli, schemas/JSON/manifests/v1.10.0). Three files make one version:
`*.yaml` version, installer, and default locale.

Placeholders in `<angle brackets>` are filled by the release workflow:
- `<VERSION>`: the release version, for example 0.1.0
- `<URL>`: the HTTPS URL of the Windows zip archive on the GitHub release
- `<SHA256>`: SHA256 of that zip, from the release's SHA256SUMS file

Install scope is `user`, so no administrator rights are needed. The zip contains the
three executables under a folder; `NestedInstallerFiles` lists them so winget places
them on PATH.

Submission to microsoft/winget-pkgs is a separate step done under your GitHub account.
Nothing here has been submitted.
