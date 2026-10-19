# Native API helpers for disposable VM validation. The service does not invoke PowerShell.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not ('Quad9Validation.DnsApi' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace Quad9Validation {
    [StructLayout(LayoutKind.Sequential)] public struct Settings {
        public uint Version; public ulong Flags;
        public IntPtr Domain, NameServer, SearchList;
        public uint RegistrationEnabled, RegisterAdapterName, EnableLLMNR, QueryAdapterName;
        public IntPtr ProfileNameServer;
        public uint DisableUnconstrainedQueries;
        public IntPtr SupplementalSearchList;
        public uint ServerCount; public IntPtr Servers;
        public uint ProfileCount; public IntPtr ProfileServers;
    }
    [StructLayout(LayoutKind.Sequential)] public struct Property {
        public uint Version, Index; public int Type; public IntPtr Data;
    }
    [StructLayout(LayoutKind.Sequential)] public struct Doh {
        public IntPtr Template; public ulong Flags;
    }
    public sealed class DohValue {
        public uint Version { get; set; }
        public uint Index { get; set; }
        public int Type { get; set; }
        public string Template { get; set; }
        public ulong Flags { get; set; }
    }
    public sealed class Snapshot {
        public uint Code { get; set; }
        public ulong Flags { get; set; }
        public string NameServer { get; set; }
        public string ProfileNameServer { get; set; }
        public DohValue[] Properties { get; set; }
        public uint ProfileCount { get; set; }
    }
    public static class DnsApi {
        [DllImport("iphlpapi.dll")] static extern uint GetInterfaceDnsSettings(Guid id, ref Settings settings);
        [DllImport("iphlpapi.dll")] static extern uint SetInterfaceDnsSettings(Guid id, ref Settings settings);
        [DllImport("iphlpapi.dll")] static extern void FreeInterfaceDnsSettings(ref Settings settings);
        public static Snapshot Read(Guid id, bool ipv6) {
            var settings = new Settings { Version = 3, Flags = ipv6 ? 1UL : 0UL };
            uint code = GetInterfaceDnsSettings(id, ref settings);
            if (code != 0) return new Snapshot { Code = code, Properties = new DohValue[0] };
            try {
                var properties = new List<DohValue>();
                if (settings.ServerCount > 128) throw new InvalidOperationException("Unexpected property count");
                for (int i = 0; i < settings.ServerCount; i++) {
                    var property = Marshal.PtrToStructure<Property>(IntPtr.Add(settings.Servers, i * Marshal.SizeOf<Property>()));
                    if (property.Type != 1 || property.Version != 1 || property.Data == IntPtr.Zero)
                        throw new InvalidOperationException("Unsupported DNS property");
                    var doh = Marshal.PtrToStructure<Doh>(property.Data);
                    properties.Add(new DohValue { Version = property.Version, Index = property.Index, Type = property.Type,
                        Template = Marshal.PtrToStringUni(doh.Template), Flags = doh.Flags });
                }
                return new Snapshot { Code = 0, Flags = settings.Flags, NameServer = Marshal.PtrToStringUni(settings.NameServer),
                    ProfileNameServer = Marshal.PtrToStringUni(settings.ProfileNameServer),
                    Properties = properties.ToArray(), ProfileCount = settings.ProfileCount };
            } finally { FreeInterfaceDnsSettings(ref settings); }
        }
        public static void Write(Guid id, bool ipv6, string servers, DohValue[] values) {
            var allocations = new List<IntPtr>();
            try {
                var settings = new Settings { Version = 3, Flags = 2UL | (ipv6 ? 1UL : 0UL),
                    NameServer = Marshal.StringToHGlobalUni(servers ?? "") };
                allocations.Add(settings.NameServer);
                if (!String.IsNullOrWhiteSpace(servers)) {
                    settings.Flags |= 0x1000;
                    settings.ServerCount = (uint)values.Length;
                    if (values.Length > 0) {
                        settings.Servers = Marshal.AllocHGlobal(values.Length * Marshal.SizeOf<Property>());
                        allocations.Add(settings.Servers);
                    }
                    for (int i = 0; i < values.Length; i++) {
                        var value = values[i];
                        var template = value.Template == null ? IntPtr.Zero : Marshal.StringToHGlobalUni(value.Template);
                        if (template != IntPtr.Zero) allocations.Add(template);
                        var doh = Marshal.AllocHGlobal(Marshal.SizeOf<Doh>());
                        allocations.Add(doh);
                        Marshal.StructureToPtr(new Doh { Template = template, Flags = value.Flags }, doh, false);
                        Marshal.StructureToPtr(new Property { Version = 1, Index = value.Index, Type = 1, Data = doh },
                            IntPtr.Add(settings.Servers, i * Marshal.SizeOf<Property>()), false);
                    }
                }
                uint code = SetInterfaceDnsSettings(id, ref settings);
                if (code != 0) throw new InvalidOperationException("SetInterfaceDnsSettings returned " + code);
            } finally { for (int i = allocations.Count - 1; i >= 0; i--) Marshal.FreeHGlobal(allocations[i]); }
        }
    }
}
'@
}

function ConvertTo-DohValues {
    param([object[]] $Properties)
    $result = [Collections.Generic.List[Quad9Validation.DohValue]]::new()
    foreach ($property in $Properties) {
        $result.Add([Quad9Validation.DohValue]@{
            Version = $property.Version; Index = $property.Index; Type = $property.Type
            Template = $property.Template; Flags = $property.Flags
        })
    }
    return ,$result.ToArray()
}

function Restore-ValidationBaseline {
    param([object] $Baseline)
    foreach ($family in @($Baseline.families)) {
        [Quad9Validation.DnsApi]::Write([guid]$Baseline.adapter_id, [bool]$family.ipv6,
            $family.snapshot.NameServer, (ConvertTo-DohValues @($family.snapshot.Properties)))
    }
}
