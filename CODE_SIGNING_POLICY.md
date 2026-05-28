# Code Signing Policy

Windows releases of **Agents Commander** are digitally signed to ensure authenticity and integrity.

## Certificate

- **Issued by**: SignPath Foundation
- **Algorithm**: SHA256
- **Private key storage**: SignPath Hardware Security Module (HSM)

All signing requests require manual approval. The private key never leaves the HSM.

## Team

| Role | Member | Responsibility |
|------|--------|----------------|
| **Author** | [Mariano Blua](https://github.com/mblua) | Source code maintenance and development |
| **Approver** | [Mariano Blua](https://github.com/mblua) | Signing request approval |

## Verification

You can verify the digital signature of any `.exe` or `.msi` file:

**Windows Explorer**: Right-click the file > Properties > Digital Signatures tab

**PowerShell**:
```powershell
Get-AuthenticodeSignature "Agents Commander_x64-setup.exe"
```

## Privacy

This program does not transmit data to any networked system unless the user enables an opt-in feature (Telegram Bridge, Voice-to-Text). See [PRIVACY.md](PRIVACY.md) for the canonical statement and data flow.

## Attribution

Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).
