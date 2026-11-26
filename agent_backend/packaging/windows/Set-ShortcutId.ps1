# Assign a stable toast identity to the installed Start menu shortcut.
param([Parameter(Mandatory)][string]$Path)
$ErrorActionPreference = 'Stop'

if (-not ('JIsjtu.ShortcutIdentity' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;

namespace JIsjtu {
    [ComImport, Guid("00021401-0000-0000-C000-000000000046")]
    class ShellLink {}

    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    struct PropertyKey {
        public Guid format;
        public uint id;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct PropVariant {
        public ushort type, reserved1, reserved2, reserved3;
        public IntPtr value, padding;
    }

    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPropertyStore {
        void GetCount(out uint count);
        void GetAt(uint index, out PropertyKey key);
        void GetValue(ref PropertyKey key, out PropVariant value);
        void SetValue(ref PropertyKey key, ref PropVariant value);
        void Commit();
    }

    public static class ShortcutIdentity {
        public static void Set(string path, string appId) {
            var link = new ShellLink();
            var file = (IPersistFile)link;
            var store = (IPropertyStore)link;
            // PKEY_AppUserModel_ID and VT_LPWSTR, as defined in the Windows SDK.
            var key = new PropertyKey {
                format = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), id = 5
            };
            var text = new PropVariant { type = 31, value = Marshal.StringToCoTaskMemUni(appId) };
            try {
                file.Load(path, 2); // STGM_READWRITE
                store.SetValue(ref key, ref text);
                store.Commit();
                file.Save(path, true);
            }
            finally {
                Marshal.FreeCoTaskMem(text.value);
                Marshal.FinalReleaseComObject(link);
            }
        }
    }
}
'@
}
[JIsjtu.ShortcutIdentity]::Set($Path, 'JIsjtu.Desktop')
