---
name: rtk-install-windows
description: Cómo instalar RTK (Rust Token Killer) en Windows copiando rtk.exe a una carpeta que ya está en el PATH de usuario por defecto.
when_to_use: Solo cuando se pida instalar, reinstalar, actualizar o ubicar el binario de rtk en Windows, o cuando `rtk` no se encuentre en el PATH. No aplica a ningún otro binario.
---

# Instalar RTK en Windows

No hay instalador: la instalación es dejar `rtk.exe` en una carpeta que ya esté en el PATH.

## Carpeta destino

`%LOCALAPPDATA%\Microsoft\WindowsApps` (ej. `C:\Users\<user>\AppData\Local\Microsoft\WindowsApps`).

- Windows 10/11 la agrega por defecto al PATH **de usuario**, en toda cuenta. No requiere admin ni tocar variables de entorno.
- NO confundir con `C:\Program Files\WindowsApps`: esa es de paquetes MSIX, está protegida (TrustedInstaller) y **no** está en el PATH.

## Pasos

1. Descargar el asset de Windows desde https://github.com/rtk-ai/rtk/releases/tag/latest (única fuente válida) y extraer `rtk.exe` si viene comprimido.
2. Copiar: `Copy-Item rtk.exe "$env:LOCALAPPDATA\Microsoft\WindowsApps\rtk.exe" -Force`
   Para actualizar, cerrar antes procesos que usen rtk (el .exe en uso no se puede sobrescribir).
3. Abrir una terminal nueva y verificar: `where.exe rtk` y `rtk --version`.

## Si no aparece en el PATH

Comprobar que la carpeta sigue en el PATH de usuario: `[Environment]::GetEnvironmentVariable('Path','User')`. Si alguien la quitó, reagregarla ahí (no en el PATH de sistema).

Uso y subcomandos de rtk: ver skill `rtk`.
