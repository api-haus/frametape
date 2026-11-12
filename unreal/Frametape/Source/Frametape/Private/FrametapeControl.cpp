#include "FrametapeControl.h"
#include "Containers/Ticker.h"
#include "Dom/JsonObject.h"
#include "HAL/FileManager.h"
#include "Misc/Paths.h"
#include "Serialization/JsonReader.h"
#include "Serialization/JsonSerializer.h"
#include "frametape.h"

struct FFrametapeControl::FImpl
{
    TFunction<FString(const FString&, const FString&)> Start;
    TFunction<FString()> Stop;
    TFunction<bool()> IsBusy;
    FString Path;
    FString Completed;
    FString Failure;
    FTSTicker::FDelegateHandle TickHandle;
#if PLATFORM_LINUX
    ft_control* Host = nullptr;
    bool Tick(float)
    {
        const char* Raw = ft_control_poll(Host);
        if (!Raw) return true;
        TSharedPtr<FJsonObject> Request;
        FString Error;
        if (!FJsonSerializer::Deserialize(TJsonReaderFactory<>::Create(UTF8_TO_TCHAR(Raw)), Request)) Error = TEXT("Invalid control request");
        else
        {
            FString Op, Requested;
            Request->TryGetStringField(TEXT("op"), Op);
            Request->TryGetStringField(TEXT("path"), Requested);
            if (Op == TEXT("start"))
            {
                if (!Path.IsEmpty() || IsBusy()) Error = TEXT("App is already recording");
                else if (FPaths::IsRelative(Requested) || Requested.IsEmpty() || IFileManager::Get().FileExists(*Requested)) Error = TEXT("Requested archive must be an unused absolute path");
                else { FString Title; Request->TryGetStringField(TEXT("window_title"), Title); Error = Start(Requested, Title); if (Error.IsEmpty()) { Path = Requested; Failure.Empty(); } }
            }
            else if (Op == TEXT("stop"))
            {
                if (Path.IsEmpty() && Requested == Completed && !Completed.IsEmpty()) Error = Failure.IsEmpty() && !IFileManager::Get().FileExists(*Completed) ? TEXT("Completed archive is missing") : Failure;
                else if (Path.IsEmpty() || Requested != Path) Error = TEXT("This controller does not own that take");
                else
                {
                    Error = Stop();
                    if (Error.IsEmpty() && !IFileManager::Get().FileExists(*Path)) Error = TEXT("Native recorder did not publish its archive");
                    Failure = Error;
                    if (!Error.IsEmpty() && !IsBusy()) { Completed = Path; Path.Empty(); }
                }
            }
            else if (Op != TEXT("status")) Error = TEXT("Unknown recording operation");
            if (Op == TEXT("status")) Error = Failure;
            // A successful stop identifies the completed archive even after ownership clears.
            auto Reply = MakeShared<FJsonObject>();
            Reply->SetBoolField(TEXT("recording"), !Path.IsEmpty() && IsBusy());
            const FString ReplyPath = Path.IsEmpty() ? Completed : Path;
            if (ReplyPath.IsEmpty()) Reply->SetField(TEXT("path"), MakeShared<FJsonValueNull>());
            else Reply->SetStringField(TEXT("path"), ReplyPath);
            if (Error.IsEmpty()) Reply->SetField(TEXT("error"), MakeShared<FJsonValueNull>());
            else Reply->SetStringField(TEXT("error"), Error);
            FString Json; FJsonSerializer::Serialize(Reply, TJsonWriterFactory<>::Create(&Json));
            ft_control_reply(Host, TCHAR_TO_UTF8(*Json));
            if (Op == TEXT("stop") && Error.IsEmpty() && !Path.IsEmpty()) { Completed = Path; Path.Empty(); }
            return true;
        }
        ft_control_reply(Host, "{\"recording\":false,\"path\":null,\"error\":\"Invalid control request\"}");
        return true;
    }
#endif
};

FFrametapeControl::FFrametapeControl(TFunction<FString(const FString&, const FString&)> Start, TFunction<FString()> Stop, TFunction<bool()> IsBusy) : Impl(MakeUnique<FImpl>())
{
    Impl->Start = MoveTemp(Start); Impl->Stop = MoveTemp(Stop); Impl->IsBusy = MoveTemp(IsBusy);
#if PLATFORM_LINUX
    Impl->Host = ft_control_open();
    if (Impl->Host) Impl->TickHandle = FTSTicker::GetCoreTicker().AddTicker(FTickerDelegate::CreateRaw(Impl.Get(), &FImpl::Tick));
    else UE_LOG(LogTemp, Warning, TEXT("Frametape control unavailable: %s"), UTF8_TO_TCHAR(ft_last_error()));
#endif
}

FFrametapeControl::~FFrametapeControl()
{
    FTSTicker::GetCoreTicker().RemoveTicker(Impl->TickHandle);
    if (!Impl->Path.IsEmpty()) Impl->Stop();
#if PLATFORM_LINUX
    ft_control_close(Impl->Host);
#endif
}
