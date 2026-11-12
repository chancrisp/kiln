# Code signing policy

Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).

Kiln's releases are built and signed by the workflow `.github/workflows/release.yml`, on GitHub's own runners, from the source in this repository. Every release is approved by hand before it is signed. How it works is in [packaging/SIGNING.md](../packaging/SIGNING.md).

From the first signed release, Kiln's own files are signed: `kiln-app.exe`, `kiln.exe`, `lame_enc.dll` (Kiln's build of the LAME encoder, if SignPath Foundation agrees it counts as Kiln's) and the MSI installer. The FFmpeg build that comes with Kiln is another project's, and its files are not signed with Kiln's certificate.

## Team roles

- Committers and reviewers: [chancrisp](https://github.com/chancrisp)
- Approvers: [chancrisp](https://github.com/chancrisp)

## Privacy

This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it. Kiln contacts GitHub only when you choose **Check for updates** in Settings. What Kiln keeps on your PC is described in [PRIVACY.md](../PRIVACY.md).
