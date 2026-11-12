#include "FrametapeRecorder.h"
#include "FrametapeControl.h"
#include "Widgets/SWindow.h"
#include "Containers/Ticker.h"
#include "HAL/IConsoleManager.h"
#include "Misc/Paths.h"
#include "Misc/CommandLine.h"
#include "Misc/Parse.h"
#include "Engine/GameEngine.h"
#include "Modules/ModuleManager.h"

class FFrametapeExampleModule : public FDefaultGameModuleImpl
{
    FFrametapeRecorder Recorder;
    TUniquePtr<FFrametapeControl> Control;
    IConsoleObject* Start = nullptr;
    IConsoleObject* Stop = nullptr;
    FTSTicker::FDelegateHandle Tick;
    uint64 Frame = 0;
    FString VideoSmoke;
    double VideoDeadline = 0;
    bool bVideoStarted = false;
public:
    void StartupModule() override
    {
        if (!IsRunningCommandlet()) Control = MakeUnique<FFrametapeControl>([this](const FString& Path, const FString& Title)
        {
            SWindow* Window = FFrametapeRecorder::DefaultWindow();
            if (!Window || Window->GetTitle().ToString() != Title) return FString(TEXT("Requested window is not this recorder's target"));
            FFrametapeOptions Options; Options.Window = Window;
            if (!Recorder.Start(Path, TEXT("{\"host\":\"Unreal native control example\"}"), Options)) return Recorder.Error();
            Frame = 0; return FString();
        }, [this] { return Recorder.Stop() ? FString() : Recorder.Error(); }, [this] { return Recorder.IsRecording(); });
        Start = IConsoleManager::Get().RegisterConsoleCommand(TEXT("Frametape.Start"), TEXT("Frametape.Start [archive.ftape]"), FConsoleCommandWithArgsDelegate::CreateLambda([this](const TArray<FString>& Args)
        {
            const FString Path = Args.Num() ? Args[0] : FPaths::ProjectSavedDir() / FDateTime::Now().ToString(TEXT("%Y%m%d-%H%M%S.ftape"));
            if (Recorder.Start(Path, TEXT("{\"host\":\"Unreal example\"}"))) Frame = 0;
            else UE_LOG(LogTemp, Error, TEXT("Frametape: %s"), *Recorder.Error());
        }));
        Stop = IConsoleManager::Get().RegisterConsoleCommand(TEXT("Frametape.Stop"), TEXT("Finish current recording"), FConsoleCommandDelegate::CreateLambda([this]
        {
            if (Recorder.IsRecording() && !Recorder.Stop()) UE_LOG(LogTemp, Error, TEXT("Frametape: %s"), *Recorder.Error());
        }));
        FParse::Value(FCommandLine::Get(), TEXT("FrametapeVideoSmoke="), VideoSmoke);
        VideoDeadline = FPlatformTime::Seconds() + 60;
        FString SmokePath;
        if (FParse::Value(FCommandLine::Get(), TEXT("FrametapeSmoke="), SmokePath))
        {
            FFrametapeOptions Options;
            Options.bVideo = Options.bInput = false;
            bool bOk = Recorder.Start(SmokePath, TEXT("{\"host\":\"Unreal plugin smoke\"}"), Options);
            for (uint64 Index = 0; bOk && Index < 3; ++Index)
            {
                bOk = Recorder.Frame(Index, 16666667) && Recorder.Event(TEXT("custom.smoke"), FString::Printf(TEXT("{\"render_frame\":%llu}"), Index));
            }
            if (Recorder.IsRecording()) bOk = Recorder.Stop() && bOk;
            UE_LOG(LogTemp, Display, TEXT("[FRAMETAPE_SMOKE] %s %s"), bOk ? TEXT("PASS") : TEXT("FAIL"), *Recorder.Error());
        }
        Tick = FTSTicker::GetCoreTicker().AddTicker(FTickerDelegate::CreateLambda([this](float Seconds)
        {
            if (!VideoSmoke.IsEmpty())
            {
                if (FPlatformTime::Seconds() > VideoDeadline)
                {
                    if (Recorder.IsRecording()) Recorder.Stop();
                    UE_LOG(LogTemp, Error, TEXT("[FRAMETAPE_VIDEO_SMOKE] FAIL timeout %s"), *Recorder.Error());
                    FPlatformMisc::RequestExitWithStatus(false, 1);
                    return false;
                }
                if (!bVideoStarted)
                {
                    UGameEngine* Game = Cast<UGameEngine>(GEngine);
                    if (!Game || !Game->GameViewportWindow.IsValid()) return true;
                    FFrametapeOptions Options;
                    Options.bInput = false;
                    FParse::Value(FCommandLine::Get(), TEXT("FrametapeVideoFps="), Options.VideoFps);
                    Options.Window = Game->GameViewportWindow.Pin().Get();
                    if (!Recorder.Start(VideoSmoke, TEXT("{\"host\":\"Unreal video smoke\",\"input_enabled\":false}"), Options))
                    {
                        UE_LOG(LogTemp, Error, TEXT("[FRAMETAPE_VIDEO_SMOKE] FAIL %s"), *Recorder.Error());
                        FPlatformMisc::RequestExitWithStatus(false, 1);
                        return false;
                    }
                    bVideoStarted = true;
                    Frame = 0;
                }
                if (Frame < 8)
                {
                    GEngine->AddOnScreenDebugMessage(1234, 5, FColor::Green, FString::Printf(TEXT("Frametape native frame %llu"), Frame));
                    if (!Recorder.Frame(Frame++, uint64(Seconds * 1e9)))
                    {
                        UE_LOG(LogTemp, Error, TEXT("[FRAMETAPE_VIDEO_SMOKE] FAIL %s"), *Recorder.Error());
                        FPlatformMisc::RequestExitWithStatus(false, 1);
                        return false;
                    }
                }
                else if (Recorder.VideoFrames() >= 8)
                {
                    const bool bOk = Recorder.Stop();
                    UE_LOG(LogTemp, Display, TEXT("[FRAMETAPE_VIDEO_SMOKE] %s %s"), bOk ? TEXT("PASS") : TEXT("FAIL"), *Recorder.Error());
                    FPlatformMisc::RequestExitWithStatus(false, bOk ? 0 : 1);
                    return false;
                }
            }
            else if (Recorder.IsRecording()) Recorder.Frame(Frame++, uint64(Seconds * 1e9));
            return true;
        }));
    }
    void ShutdownModule() override
    {
        Control.Reset();
        FTSTicker::GetCoreTicker().RemoveTicker(Tick);
        IConsoleManager::Get().UnregisterConsoleObject(Start);
        IConsoleManager::Get().UnregisterConsoleObject(Stop);
        if (Recorder.IsRecording()) Recorder.Stop();
    }
};
IMPLEMENT_PRIMARY_GAME_MODULE(FFrametapeExampleModule, FrametapeExample, "FrametapeExample")
