# Signing Kiln's releases

Kiln's releases are signed only by `.github/workflows/release.yml`, on GitHub's own runners. No certificate or key is ever on a developer's PC, and `package.ps1` signs nothing.

The repository variable `KILN_SIGNING` chooses the service:

| `KILN_SIGNING` | Service | Who the signature names |
|---|---|---|
| `signpath` | SignPath Foundation (the plan) | SignPath Foundation |
| `azure` | Azure Artifact Signing (the fallback) | Chandler Crisp |
| not set | Nothing is signed | — |

A tag stops unless it is signed. The one exception is the tag named in the variable `KILN_UNSIGNED_RELEASE`: SignPath Foundation takes only projects that have released already, so `v0.1.0` goes out unsigned. Clear that variable once `v0.1.0` is published.

## What is signed

Signing happens in two rounds:

1. **The staged files.** `kiln-app.exe`, `kiln.exe` and `lame_enc.dll` are signed in `dist\stage`, before any package holds them.
2. **The MSI.** It is built from those signed files, then signed itself.

The portable ZIP is made from the signed files. The source, FFmpeg source and symbols ZIPs hold no programs that run, so they are not signed. `SHA256SUMS.txt` lists them all.

**FFmpeg isn't signed.** FFmpeg's `ffmpeg.exe`, `ffprobe.exe` and DLLs are another project's binaries. SignPath Foundation's terms don't allow Kiln to sign them, so they ship unsigned inside the signed MSI and ZIP. `packaging\check-signatures.ps1` lists them as `upstream`.

**`lame_enc.dll` may not be signed.** It is LAME's code, built by Kiln's kiln-lame crate. It is signed only if SignPath Foundation agrees that it counts as Kiln's own build. If they don't, three changes go together:
1. Delete its `pe-file` element from `packaging\signpath\kiln-files.xml`.
2. In `packaging\check-signatures.ps1`, change the defaults so the check lists it as upstream:
   - add `'.'` (the top folder) to `-Upstream`;
   - add `|^lame_enc\.dll$` to the end of `-UpstreamNames`.
3. Change the check in `crates/kiln-core/tests/packaging.rs` (`the_package_script_builds_and_stages_the_mp3_encoder`) that it is in `kiln-files.xml`.

**The check.** After each round, `packaging\check-signatures.ps1` stops the job unless every Kiln program, DLL and MSI has a `Valid` Authenticode signature. With `-Packages` it also checks the files inside the portable ZIP and inside the MSI (unpacked with `msiexec /a`, which installs nothing).

## SignPath Foundation

What SignPath Foundation asks of a project, from https://signpath.org/terms (read 2026-11-10):

- an OSI licence for all of it (GPL-3.0-or-later), and no proprietary parts;
- a project that is maintained, already released, and described on its download page;
- every team member uses multi-factor authentication on GitHub and on SignPath;
- a "Code signing policy" on the home page and the release pages: `docs/code-signing-policy.md`, linked from the README;
- every signed file's product name set to the project's, and its product version the same in each build. `kiln-files.xml` enforces both: the workflow passes the version as the parameter `version`;
- every release is approved by hand in SignPath before it is signed.

Setting it up, once SignPath Foundation accepts Kiln:

1. In the SignPath project, add GitHub.com as a trusted build system, linked to `chancrisp/kiln`.
2. Add two artifact configurations, pasting in the files from this folder:
   - `kiln-files`, from `signpath\kiln-files.xml`;
   - `kiln-msi`, from `signpath\kiln-msi.xml`.
3. Use the signing policies SignPath sets up:
   - `test-signing`, which a run by hand (a dry run) uses, with SignPath's test certificate;
   - `release-signing`, which a tag uses, approved by hand.
   The workflow picks the policy by itself, by whether the run is for a tag.
4. Create a CI user with the Submitter role on both policies, and make an API token for it.
5. In the GitHub repository, under Settings > Secrets and variables > Actions:
   - the secret `SIGNPATH_API_TOKEN`: that token;
   - the variable `SIGNPATH_ORGANIZATION_ID`: the organisation ID from SignPath;
   - the variable `SIGNPATH_PROJECT_SLUG`: the project's slug (for example `kiln`);
   - the variable `KILN_SIGNING`: `signpath`.
6. For a test signature, run Release by hand: Actions > Release > Run workflow. Off a tag, the check accepts SignPath's test certificate, which Windows doesn't trust; on a tag it never does. SignPath's wait for approval is up to two hours a round.

## Azure Artifact Signing (fallback)

Use it if SignPath Foundation turns Kiln down or takes too long. It costs $9.99 a month (Basic), and an individual can use it in the US or Canada. The signature then names the identity Microsoft verifies, so it should be Chandler Crisp, the publisher on the MSI and the programs.

1. In the Azure portal, create an Artifact Signing account and verify an individual identity. Then create a Public Trust certificate profile.
2. Create an app registration (Microsoft Entra ID). Give it the role "Artifact Signing Certificate Profile Signer" on the account. Add a federated credential for GitHub Actions:
   - organisation `chancrisp`, repository `kiln`;
   - entity: environment, named `release`. The package job runs in that environment, so one credential covers every tag, and GitHub can make it wait for a reviewer (Settings > Environments > release).
3. In the GitHub repository:
   - the secrets `AZURE_TENANT_ID`, `AZURE_CLIENT_ID` and `AZURE_SUBSCRIPTION_ID`, from the app registration and the subscription. There is no password: GitHub signs in through OpenID Connect;
   - the variable `AZURE_SIGNING_ENDPOINT`: the account's region endpoint, for example `https://eus.codesigning.azure.net/`;
   - the variables `AZURE_SIGNING_ACCOUNT` and `AZURE_CERTIFICATE_PROFILE`: the names from step 1;
   - the variable `KILN_SIGNING`: `azure`.

Switching back to SignPath is one variable: set `KILN_SIGNING` to `signpath`.
