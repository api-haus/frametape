using UnrealBuildTool;
public class FrametapeExampleEditorTarget : TargetRules
{
    public FrametapeExampleEditorTarget(TargetInfo Target) : base(Target)
    {
        Type = TargetType.Editor;
        DefaultBuildSettings = BuildSettingsVersion.Latest;
        IncludeOrderVersion = EngineIncludeOrderVersion.Latest;
        ExtraModuleNames.Add("FrametapeExample");
    }
}
