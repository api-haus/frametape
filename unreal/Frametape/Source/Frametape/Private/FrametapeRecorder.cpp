#include "FrametapeRecorder.h"
#include "frametape.h"

#include "Dom/JsonObject.h"
#include "Engine/GameEngine.h"
#include "Framework/Application/IInputProcessor.h"
#include "Framework/Application/SlateApplication.h"
#include "HAL/FileManager.h"
#include "Misc/Paths.h"
#include "Misc/CoreDelegates.h"
#include "Misc/ScopeLock.h"
#include "RHICommandList.h"
#include "RHIGPUReadback.h"
#include "Rendering/SlateRenderer.h"
#include "RenderingThread.h"
#include "Serialization/JsonSerializer.h"
#include "Slate/SlateViewportProvider.h"
#include <atomic>

namespace
{
    FString Json(const TSharedRef<FJsonObject>& Object)
    {
        FString Text;
        FJsonSerializer::Serialize(Object, TJsonWriterFactory<>::Create(&Text));
        return Text;
    }

    struct FSession
    {
        FCriticalSection Mutex;
        ft_session* Handle = nullptr;
        uint64 Index = MAX_uint64;
        uint64 LastStart = 0;
        double Epoch = FPlatformTime::Seconds();
        FString Failure;

        uint64 Now() const { return uint64((FPlatformTime::Seconds() - Epoch) * 1e9); }
        void Failed() { Failure = UTF8_TO_TCHAR(ft_last_error()); }
        bool Emit(const FString& Channel, const FString& Data, uint64 Frame = MAX_uint64)
        {
            FScopeLock Lock(&Mutex);
            if (!Handle) return false;
            if (ft_session_event(Handle, Frame, Now(), TCHAR_TO_UTF8(*Channel), TCHAR_TO_UTF8(*Data)) != 0)
            {
                Failed();
                return false;
            }
            return true;
        }
    };

    class FInput final : public IInputProcessor
    {
        TSharedRef<FSession, ESPMode::ThreadSafe> Session;
        bool Key(const TCHAR* Kind, const FKeyEvent& Event)
        {
            TSharedRef<FJsonObject> Data = MakeShared<FJsonObject>();
            Data->SetStringField(TEXT("kind"), Kind);
            Data->SetStringField(TEXT("key"), Event.GetKey().GetFName().ToString());
            Data->SetNumberField(TEXT("user"), Event.GetUserIndex());
            Data->SetBoolField(TEXT("repeat"), Event.IsRepeat());
            Data->SetNumberField(TEXT("key_code"), Event.GetKeyCode());
            Data->SetNumberField(TEXT("character_code"), Event.GetCharacter());
            Data->SetBoolField(TEXT("shift"), Event.IsShiftDown());
            Data->SetBoolField(TEXT("control"), Event.IsControlDown());
            Data->SetBoolField(TEXT("alt"), Event.IsAltDown());
            Data->SetBoolField(TEXT("command"), Event.IsCommandDown());
            Session->Emit(TEXT("input"), Json(Data));
            return false;
        }
        bool Pointer(const TCHAR* Kind, const FPointerEvent& Event)
        {
            TSharedRef<FJsonObject> Data = MakeShared<FJsonObject>();
            const FVector2D Position = Event.GetScreenSpacePosition(), Delta = Event.GetCursorDelta();
            Data->SetStringField(TEXT("kind"), Kind);
            Data->SetStringField(TEXT("button"), Event.GetEffectingButton().GetFName().ToString());
            Data->SetNumberField(TEXT("pointer"), Event.GetPointerIndex());
            Data->SetBoolField(TEXT("touch"), Event.IsTouchEvent());
            Data->SetNumberField(TEXT("x"), Position.X);
            Data->SetNumberField(TEXT("y"), Position.Y);
            Data->SetNumberField(TEXT("dx"), Delta.X);
            Data->SetNumberField(TEXT("dy"), Delta.Y);
            Data->SetNumberField(TEXT("wheel"), Event.GetWheelDelta());
            Session->Emit(TEXT("input"), Json(Data));
            return false;
        }
    public:
        explicit FInput(TSharedRef<FSession, ESPMode::ThreadSafe> InSession) : Session(InSession) {}
        void Tick(float, FSlateApplication&, TSharedRef<ICursor>) override {}
        bool HandleKeyDownEvent(FSlateApplication&, const FKeyEvent& E) override { return Key(TEXT("key_down"), E); }
        bool HandleKeyUpEvent(FSlateApplication&, const FKeyEvent& E) override { return Key(TEXT("key_up"), E); }
        bool HandleAnalogInputEvent(FSlateApplication&, const FAnalogInputEvent& E) override
        {
            TSharedRef<FJsonObject> Data = MakeShared<FJsonObject>();
            Data->SetStringField(TEXT("kind"), TEXT("axis"));
            Data->SetStringField(TEXT("key"), E.GetKey().GetFName().ToString());
            Data->SetNumberField(TEXT("value"), E.GetAnalogValue());
            Data->SetNumberField(TEXT("user"), E.GetUserIndex());
            Session->Emit(TEXT("input"), Json(Data));
            return false;
        }
        bool HandleMouseMoveEvent(FSlateApplication&, const FPointerEvent& E) override { return Pointer(TEXT("pointer_move"), E); }
        bool HandleMouseButtonDownEvent(FSlateApplication&, const FPointerEvent& E) override { return Pointer(TEXT("pointer_down"), E); }
        bool HandleMouseButtonUpEvent(FSlateApplication&, const FPointerEvent& E) override { return Pointer(TEXT("pointer_up"), E); }
        bool HandleMouseButtonDoubleClickEvent(FSlateApplication&, const FPointerEvent& E) override { return Pointer(TEXT("pointer_double"), E); }
        bool HandleMouseWheelOrGestureEvent(FSlateApplication&, const FPointerEvent& E, const FPointerEvent* Gesture) override
        {
            Pointer(TEXT("wheel"), E);
            if (Gesture) Pointer(TEXT("gesture"), *Gesture);
            return false;
        }
        bool HandleMotionDetectedEvent(FSlateApplication&, const FMotionEvent& E) override
        {
            const FVector Tilt = E.GetTilt(), Rotation = E.GetRotationRate(), Gravity = E.GetGravity(), Acceleration = E.GetAcceleration();
            Session->Emit(TEXT("input"), FString::Printf(TEXT("{\"kind\":\"motion\",\"tilt\":[%f,%f,%f],\"rotation_rate\":[%f,%f,%f],\"gravity\":[%f,%f,%f],\"acceleration\":[%f,%f,%f]}"), Tilt.X, Tilt.Y, Tilt.Z, Rotation.X, Rotation.Y, Rotation.Z, Gravity.X, Gravity.Y, Gravity.Z, Acceleration.X, Acceleration.Y, Acceleration.Z));
            return false;
        }
    };

    struct FVideo
    {
        TSharedRef<FSession, ESPMode::ThreadSafe> Session;
        SWindow* Window = nullptr;
        FString Path;
        uint32 Fps = 60;
        FDelegateHandle Presented;
        TOptional<uint64> Wanted;
        TArray<TPair<uint64, uint64>> Queued;
        struct FSlot { TUniquePtr<FRHIGPUTextureReadback> Readback; uint64 Index = MAX_uint64; FIntPoint Size; };
        TArray<FSlot> Slots;
        TArray<int32> InFlight;
        FIntPoint Size = FIntPoint::ZeroValue;
        ft_video* Encoder = nullptr;
        bool bFailed = false;
        std::atomic<uint64> Accepted{0};

        explicit FVideo(TSharedRef<FSession, ESPMode::ThreadSafe> InSession) : Session(InSession) { Slots.SetNum(8); }
        void Status(uint64 Frame, const TCHAR* Status, uint64 PictureIndex = MAX_uint64)
        {
            const FString Picture = PictureIndex != MAX_uint64 ? FString::Printf(TEXT("%llu"), PictureIndex) : TEXT("null");
            Session->Emit(TEXT("video.frame"), FString::Printf(TEXT("{\"status\":\"%s\",\"source_frame\":%llu,\"video_frame\":%s}"), Status, Frame, *Picture), Frame);
        }
        void Poll()
        {
            while (InFlight.Num() && Slots[InFlight[0]].Readback->IsReady())
            {
                FSlot& Slot = Slots[InFlight[0]];
                InFlight.RemoveAt(0);
                int32 Pitch = 0;
                const FColor* Data = static_cast<const FColor*>(Slot.Readback->Lock(Pitch));
                TArray<FColor> Picture;
                Picture.SetNumUninitialized(Slot.Size.X * Slot.Size.Y);
                for (int32 Y = 0; Y < Slot.Size.Y; ++Y) FMemory::Memcpy(&Picture[Y * Slot.Size.X], Data + Y * Pitch, Slot.Size.X * sizeof(FColor));
                Slot.Readback->Unlock();
                const int Result = ft_video_frame(Encoder, Slot.Index, reinterpret_cast<const uint8*>(Picture.GetData()), Picture.Num() * sizeof(FColor));
                if (Result < 0)
                {
                    FScopeLock Lock(&Session->Mutex);
                    Session->Failed();
                    bFailed = true;
                }
                uint64 PictureIndex = MAX_uint64;
                if (Result > 0)
                {
                    PictureIndex = Accepted.fetch_add(1, std::memory_order_relaxed);
                    Queued.Emplace(Slot.Index, PictureIndex);
                }
                Status(Slot.Index, Result > 0 ? TEXT("queued") : Result == 0 ? TEXT("encoder_busy") : TEXT("encoder_failed"), PictureIndex);
                Slot.Index = MAX_uint64;
            }
        }
        void OnPresent(SWindow& InWindow, ISlateViewportProvider& Provider)
        {
            if (&InWindow != Window) return;
            Poll();
            if (!Wanted.IsSet()) return;
            const uint64 Index = Wanted.GetValue();
            Wanted.Reset();
            if (bFailed) { Status(Index, TEXT("encoder_failed")); return; }
            FRHITexture* Buffer = Provider.GetBackBufferResource();
            const TCHAR* Format = !Buffer ? nullptr : Buffer->GetFormat() == PF_B8G8R8A8 ? TEXT("bgr0") : Buffer->GetFormat() == PF_R8G8B8A8 ? TEXT("rgb0") : Buffer->GetFormat() == PF_A2B10G10R10 ? TEXT("x2bgr10le") : nullptr;
            if (!Format) { Status(Index, TEXT("unsupported_format")); return; }
            const FIntPoint CurrentSize(Buffer->GetSizeX() & ~1, Buffer->GetSizeY() & ~1);
            if (!Encoder)
            {
                Encoder = ft_video_open(TCHAR_TO_UTF8(*Path), CurrentSize.X, CurrentSize.Y, Fps, TCHAR_TO_UTF8(Format));
                if (!Encoder)
                {
                    { FScopeLock Lock(&Session->Mutex); Session->Failed(); }
                    bFailed = true;
                    Status(Index, TEXT("encoder_failed"));
                    return;
                }
                Size = CurrentSize;
            }
            if (Size != CurrentSize) { Status(Index, TEXT("resized")); return; }
            const int32 Free = Slots.IndexOfByPredicate([](const FSlot& Slot) { return Slot.Index == MAX_uint64; });
            if (Free == INDEX_NONE) { Status(Index, TEXT("readback_busy")); return; }
            FSlot& Slot = Slots[Free];
            if (!Slot.Readback) Slot.Readback = MakeUnique<FRHIGPUTextureReadback>(TEXT("Frametape"));
            Slot.Index = Index;
            Slot.Size = Size;
            Slot.Readback->EnqueueCopy(FRHICommandListImmediate::Get(), Buffer, FIntVector::ZeroValue, 0, FIntVector(Size.X, Size.Y, 1));
            InFlight.Add(Free);
        }
    };
}

struct FFrametapeRecorder::FImpl
{
    TSharedRef<FSession, ESPMode::ThreadSafe> Session = MakeShared<FSession, ESPMode::ThreadSafe>();
    TSharedPtr<FVideo, ESPMode::ThreadSafe> Video;
    TSharedPtr<FInput> Input;
    FString ArchivePath;
    FDelegateHandle PreExit;
    uint64 CompletedVideoFrames = 0;
};

FFrametapeRecorder::FFrametapeRecorder() : Impl(MakeUnique<FImpl>()) {}
FFrametapeRecorder::~FFrametapeRecorder() { if (IsRecording()) Stop(); }

bool FFrametapeRecorder::Start(const FString& Path, const FString& Metadata, const FFrametapeOptions& Options)
{
    if (IsRecording()) { Impl->Session->Failure = TEXT("Already recording"); return false; }
    Impl->Session = MakeShared<FSession, ESPMode::ThreadSafe>();
    Impl->CompletedVideoFrames = 0;
    Impl->ArchivePath = FPaths::ConvertRelativePathToFull(Path);
    if (Options.bVideo && IFileManager::Get().FileExists(*FPaths::ChangeExtension(Impl->ArchivePath, TEXT("mp4"))))
    {
        Impl->Session->Failure = TEXT("Video output already exists");
        return false;
    }
    if (!Options.VideoFps) { Impl->Session->Failure = TEXT("VideoFps must be positive"); return false; }
    const bool bSlate = FSlateApplication::IsInitialized();
    if (Options.bInput && !bSlate) { Impl->Session->Failure = TEXT("Input requested but Slate is unavailable"); return false; }
    SWindow* Window = Options.Window;
    if (Options.bVideo)
    {
        if (!bSlate) { Impl->Session->Failure = TEXT("Video requested but Slate is unavailable"); return false; }
        if (!Window) Window = DefaultWindow();
        if (!Window) { Impl->Session->Failure = TEXT("Video requested but no window is available"); return false; }
    }
    TSharedPtr<FJsonObject> MetadataObject;
    if (!FJsonSerializer::Deserialize(TJsonReaderFactory<>::Create(Metadata), MetadataObject) || !MetadataObject.IsValid())
    {
        Impl->Session->Failure = TEXT("Metadata must be a JSON object");
        return false;
    }
    MetadataObject->SetNumberField(TEXT("video_fps"), Options.VideoFps);
    MetadataObject->SetBoolField(TEXT("video_requested"), Options.bVideo);
    MetadataObject->SetBoolField(TEXT("video_available"), bSlate && Window);
    MetadataObject->SetBoolField(TEXT("input_requested"), Options.bInput);
    MetadataObject->SetBoolField(TEXT("input_available"), bSlate);
    MetadataObject->SetBoolField(TEXT("input_enabled"), Options.bInput);
    MetadataObject->SetBoolField(TEXT("video_enabled"), Options.bVideo);
    const FString ActualMetadata = Json(MetadataObject.ToSharedRef());
    IFileManager::Get().MakeDirectory(*FPaths::GetPath(Impl->ArchivePath), true);
    Impl->Session->Handle = ft_session_open(TCHAR_TO_UTF8(*Impl->ArchivePath), TCHAR_TO_UTF8(*ActualMetadata));
    if (!Impl->Session->Handle) { Impl->Session->Failed(); return false; }
    Impl->PreExit = FCoreDelegates::OnEnginePreExit.AddLambda([this] { if (IsRecording()) Stop(); });
    if (Options.bInput)
    {
        Impl->Input = MakeShared<FInput>(Impl->Session);
        FSlateApplication::Get().RegisterInputPreProcessor(Impl->Input);
    }
    if (Options.bVideo)
    {
        Impl->Video = MakeShared<FVideo, ESPMode::ThreadSafe>(Impl->Session);
        Impl->Video->Window = Window;
        Impl->Video->Path = FPaths::ChangeExtension(Impl->ArchivePath, TEXT("mp4"));
        Impl->Video->Fps = Options.VideoFps;
        const auto Video = Impl->Video.ToSharedRef();
        Video->Presented = FSlateApplication::Get().GetRenderer()->OnBackBufferReadyToPresent().AddLambda([Video](SWindow& W, ISlateViewportProvider& P) { Video->OnPresent(W, P); });
    }
    return true;
}

bool FFrametapeRecorder::Frame(uint64 Index, uint64 DurationNs)
{
    auto Session = Impl->Session;
    {
        FScopeLock Lock(&Session->Mutex);
        if (!Session->Handle) return false;
        const uint64 End = Session->Now();
        const uint64 Start = FMath::Max(Session->LastStart, End > DurationNs ? End - DurationNs : 0);
        if (ft_session_frame(Session->Handle, Index, Start, DurationNs) != 0) { Session->Failed(); return false; }
        Session->Index = Index;
        Session->LastStart = Start;
    }
    if (Impl->Video)
    {
        ENQUEUE_RENDER_COMMAND(FrametapeFrame)([Video = Impl->Video, Index](FRHICommandListImmediate&)
        {
            if (Video->Wanted.IsSet()) Video->Status(Video->Wanted.GetValue(), TEXT("not_presented"));
            Video->Wanted = Index;
        });
    }
    return true;
}

bool FFrametapeRecorder::Event(const FString& Channel, const FString& Data) { return Impl->Session->Emit(Channel, Data, Impl->Session->Index); }
bool FFrametapeRecorder::Attach(const FString& Channel, const FString& Path, const FString& MediaType)
{
    auto Session = Impl->Session;
    FScopeLock Lock(&Session->Mutex);
    if (!Session->Handle) return false;
    if (ft_session_attach(Session->Handle, TCHAR_TO_UTF8(*Channel), TCHAR_TO_UTF8(*Path), TCHAR_TO_UTF8(*MediaType)) == 0) return true;
    Session->Failed();
    return false;
}

bool FFrametapeRecorder::Stop()
{
    if (!IsRecording()) return false;
    FCoreDelegates::OnEnginePreExit.Remove(Impl->PreExit);
    Impl->PreExit.Reset();
    bool bOk = true;
    if (Impl->Input && FSlateApplication::IsInitialized()) FSlateApplication::Get().UnregisterInputPreProcessor(Impl->Input);
    Impl->Input.Reset();
    if (Impl->Video)
    {
        const auto Video = Impl->Video;
        FSlateApplication::Get().GetRenderer()->OnBackBufferReadyToPresent().Remove(Video->Presented);
        ENQUEUE_RENDER_COMMAND(FrametapeDrain)([Video](FRHICommandListImmediate& Cmd)
        {
            Cmd.SubmitAndBlockUntilGPUIdle();
            Video->Poll();
            if (Video->Wanted.IsSet()) Video->Status(Video->Wanted.GetValue(), TEXT("not_presented"));
            Video->Wanted.Reset();
            if (Video->Encoder)
            {
                const int Result = ft_video_close(Video->Encoder);
                Video->Encoder = nullptr;
                if (Result != 0) { FScopeLock Lock(&Video->Session->Mutex); Video->Session->Failed(); Video->bFailed = true; }
            }
            else
            {
                FScopeLock Lock(&Video->Session->Mutex);
                Video->Session->Failure = TEXT("No video pictures captured");
                Video->bFailed = true;
            }
            if (!Video->bFailed) for (const auto& Mapping : Video->Queued) Video->Status(Mapping.Key, TEXT("captured"), Mapping.Value);
            TSharedRef<FJsonObject> Result = MakeShared<FJsonObject>();
            Result->SetBoolField(TEXT("complete"), !Video->bFailed);
            Result->SetNumberField(TEXT("pictures"), Video->Accepted.load(std::memory_order_relaxed));
            if (Video->bFailed)
            {
                FScopeLock Lock(&Video->Session->Mutex);
                Result->SetStringField(TEXT("error"), Video->Session->Failure);
            }
            Video->Session->Emit(TEXT("video.result"), Json(Result));
        });
        FlushRenderingCommands();
        Impl->CompletedVideoFrames = Video->Accepted.load(std::memory_order_relaxed);
        if (Video->bFailed)
        {
            bOk = false;
            if (IFileManager::Get().FileExists(*Video->Path)) Attach(TEXT("video.failed"), Video->Path, TEXT("application/octet-stream"));
        }
        else if (IFileManager::Get().FileExists(*Video->Path)) bOk = Attach(TEXT("video"), Video->Path, TEXT("video/mp4")) && bOk;
        Impl->Video.Reset();
    }
    auto Session = Impl->Session;
    FScopeLock Lock(&Session->Mutex);
    const int Result = ft_session_close(Session->Handle);
    Session->Handle = nullptr;
    if (Result != 0) { Session->Failed(); return false; }
    return bOk && Session->Failure.IsEmpty();
}

bool FFrametapeRecorder::IsRecording() const { return Impl->Session->Handle != nullptr; }
uint64 FFrametapeRecorder::VideoFrames() const { return Impl->Video ? Impl->Video->Accepted.load(std::memory_order_relaxed) : Impl->CompletedVideoFrames; }
FString FFrametapeRecorder::Error() const { FScopeLock Lock(&Impl->Session->Mutex); return Impl->Session->Failure; }

SWindow* FFrametapeRecorder::DefaultWindow()
{
    if (!FSlateApplication::IsInitialized()) return nullptr;
    if (UGameEngine* Game = Cast<UGameEngine>(GEngine); Game && Game->GameViewportWindow.IsValid()) return Game->GameViewportWindow.Pin().Get();
    return FSlateApplication::Get().GetActiveTopLevelWindow().Get();
}
