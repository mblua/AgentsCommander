# Code Signing Policy

Windows code signing for **Agents Commander** is planned through SignPath Foundation and is pending SignPath setup and approval.

Current Windows release artifacts may be unsigned until [epic #717](https://github.com/mblua/AgentsCommander/issues/717) is complete. Use this policy as the intended signing policy and verification reference while the SignPath application and release workflow integration are in progress.

## Planned Certificate

- **Expected issuer**: SignPath Foundation
- **Algorithm**: SHA256
- **Private key storage after approval**: SignPath Hardware Security Module (HSM)

When signing is active, every signing request will require manual approval. The private key will never leave the HSM.

## Team

| Role | Member | Responsibility |
|------|--------|----------------|
| **Author** | [Mariano Blua](https://github.com/mblua) | Source code maintenance and development |
| **Approver** | [Mariano Blua](https://github.com/mblua) | Signing request approval |

## Verification

You can inspect the Authenticode status of any Windows `.exe` or `.msi` file:

**Windows Explorer**: Right-click the file > Properties > Digital Signatures tab

**PowerShell**:
```powershell
Get-AuthenticodeSignature "Agents Commander_X.Y.Z_x64-setup.exe"
```

Until Windows signing is active, `Status` may read `NotSigned`. Once SignPath signing is active, signed artifacts should report `Valid` and identify SignPath Foundation as the signer.

For every release, also verify the downloaded file against the attached `SHASUMS256.txt` checksum file.

## Privacy

This program does not transmit data to any networked system unless the user enables an opt-in feature (Telegram Bridge, Voice-to-Text). See [PRIVACY.md](PRIVACY.md) for the canonical statement and data flow.

## Attribution

AgentsCommander intends to use free code signing provided by [SignPath.io](https://signpath.io), with certificate support by [SignPath Foundation](https://signpath.org), after approval.
