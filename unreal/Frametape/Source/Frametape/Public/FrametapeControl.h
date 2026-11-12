#pragma once
#include "CoreMinimal.h"

// Opt-in process recorder discovery. Callbacks run on the game thread; empty string means success.
// Stop must publish the requested archive before returning. Independent from video/input producers.
class FRAMETAPE_API FFrametapeControl
{
public:
    FFrametapeControl(TFunction<FString(const FString&, const FString&)> Start, TFunction<FString()> Stop, TFunction<bool()> IsBusy);
    ~FFrametapeControl();
    FFrametapeControl(const FFrametapeControl&) = delete;
    FFrametapeControl& operator=(const FFrametapeControl&) = delete;
private:
    struct FImpl;
    TUniquePtr<FImpl> Impl;
};
