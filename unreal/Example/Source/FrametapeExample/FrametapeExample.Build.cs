using UnrealBuildTool;
public class FrametapeExample : ModuleRules
{
    public FrametapeExample(ReadOnlyTargetRules Target) : base(Target)
    {
        PCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;
        PublicDependencyModuleNames.AddRange(new[] { "Core", "CoreUObject", "Engine", "Frametape" });
    }
}
