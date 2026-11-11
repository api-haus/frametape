using System;
using System.IO;
using UnrealBuildTool;

public class Frametape : ModuleRules
{
    public Frametape(ReadOnlyTargetRules Target) : base(Target)
    {
        PCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;
        PublicDependencyModuleNames.AddRange(new[] { "Core" });
        PrivateDependencyModuleNames.AddRange(new[] { "CoreUObject", "Engine", "Slate", "SlateCore", "ApplicationCore", "InputCore", "Json", "RHI", "RenderCore" });
        string ThirdParty = Path.GetFullPath(Path.Combine(ModuleDirectory, "../ThirdParty"));
        PrivateIncludePaths.Add(Path.Combine(ThirdParty, "include"));
        string Platform = Target.Platform.ToString();
        string Library = Path.Combine(ThirdParty, "lib", Platform, Platform == "Win64" ? "frametape_ffi.lib" : "libframetape_ffi.a");
        if (!File.Exists(Library)) throw new BuildException("Frametape ABI missing. Run frametape/unreal/install PROJECT before building: " + Library);
        PublicAdditionalLibraries.Add(Library);
        if (Target.Platform == UnrealTargetPlatform.Linux) PublicSystemLibraries.AddRange(new[] { "pthread", "dl", "m" });
    }
}
