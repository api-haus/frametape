#pragma once

#include "CoreMinimal.h"

class SWindow;

struct FRAMETAPE_API FFrametapeOptions
{
    bool bVideo = true;
    bool bInput = true;
    uint32 VideoFps = 60;
    SWindow* Window = nullptr;
};

// Each recorder owns its subscriptions, native encoder and journal. Calls run on the game thread.
class FRAMETAPE_API FFrametapeRecorder
{
public:
    FFrametapeRecorder();
    ~FFrametapeRecorder();
    FFrametapeRecorder(const FFrametapeRecorder&) = delete;
    FFrametapeRecorder& operator=(const FFrametapeRecorder&) = delete;

    bool Start(const FString& ArchivePath, const FString& MetadataJson, const FFrametapeOptions& Options = {});
    bool Frame(uint64 Index, uint64 DurationNs);
    bool Event(const FString& Channel, const FString& Json);
    bool Attach(const FString& Channel, const FString& Path, const FString& MediaType);
    // Drains GPU readbacks and encoding before closing the archive; returns errors instead of copying stale video.
    bool Stop();
    bool IsRecording() const;
    // Accepted pictures, finalized when Stop succeeds; independent of logical frame count.
    uint64 VideoFrames() const;
    FString Error() const;

private:
    struct FImpl;
    TUniquePtr<FImpl> Impl;
};
