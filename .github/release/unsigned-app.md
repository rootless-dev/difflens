## Installing

Download the disk image for your Mac:

- **Apple Silicon** (M1 and later): `DiffLens_<version>_aarch64.dmg`
- **Intel**: `DiffLens_<version>_x64.dmg`

Open it and drag **DiffLens** to **Applications**.

### Opening it the first time

DiffLens is not signed with an Apple Developer ID and is not notarized (it is
ad-hoc signed), so macOS blocks it the first time it is opened. Either:

1. Open DiffLens once and dismiss the warning, then go to
   **System Settings > Privacy & Security**, scroll to **Security**, and click
   **Open Anyway** next to the message about DiffLens. Confirm with your
   password.
2. Or remove the quarantine flag in Terminal:

   ```sh
   xattr -dr com.apple.quarantine /Applications/DiffLens.app
   ```

   Use this one too if macOS says the app "is damaged and can't be opened".

After that, DiffLens opens normally. There is no automatic update yet:
download new versions from this page.
