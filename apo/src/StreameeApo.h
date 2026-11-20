// Streamee APO — effet système (SFX) posé sur le micro physique.
// Il recopie le signal du micro et y ajoute les sons envoyés par l'application.
#pragma once

#include <windows.h>
#include <unknwn.h>
#include <audioenginebaseapo.h>
#include <atomic>

#include "shared.h"

// {EE91D2D7-78FB-42EB-9BB7-3190B21658D2}
static const CLSID CLSID_StreameeApo =
    { 0xee91d2d7, 0x78fb, 0x42eb, { 0x9b, 0xb7, 0x31, 0x90, 0xb2, 0x16, 0x58, 0xd2 } };

extern std::atomic<long> g_objectCount;
extern std::atomic<long> g_lockCount;

// Interface « non déléguée » pour l'agrégation COM (audiodg peut agréger l'APO).
struct INonDelegatingUnknown {
    virtual HRESULT STDMETHODCALLTYPE NonDelegatingQueryInterface(REFIID riid, void** ppv) = 0;
    virtual ULONG STDMETHODCALLTYPE NonDelegatingAddRef() = 0;
    virtual ULONG STDMETHODCALLTYPE NonDelegatingRelease() = 0;
};

class StreameeApo final
    : public INonDelegatingUnknown,
      public IAudioProcessingObject,
      public IAudioProcessingObjectRT,
      public IAudioProcessingObjectConfiguration,
      public IAudioSystemEffects2 {
public:
    explicit StreameeApo(IUnknown* outer);
    ~StreameeApo();

    // INonDelegatingUnknown
    HRESULT STDMETHODCALLTYPE NonDelegatingQueryInterface(REFIID riid, void** ppv) override;
    ULONG STDMETHODCALLTYPE NonDelegatingAddRef() override;
    ULONG STDMETHODCALLTYPE NonDelegatingRelease() override;

    // IUnknown (délègue à l'objet extérieur)
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void** ppv) override { return m_outer->QueryInterface(riid, ppv); }
    ULONG STDMETHODCALLTYPE AddRef() override { return m_outer->AddRef(); }
    ULONG STDMETHODCALLTYPE Release() override { return m_outer->Release(); }

    // IAudioProcessingObject
    HRESULT STDMETHODCALLTYPE Reset() override;
    HRESULT STDMETHODCALLTYPE GetLatency(HNSTIME* pTime) override;
    HRESULT STDMETHODCALLTYPE GetRegistrationProperties(APO_REG_PROPERTIES** ppRegProps) override;
    HRESULT STDMETHODCALLTYPE Initialize(UINT32 cbDataSize, BYTE* pbyData) override;
    HRESULT STDMETHODCALLTYPE IsInputFormatSupported(IAudioMediaType* pOppositeFormat,
        IAudioMediaType* pRequestedInputFormat, IAudioMediaType** ppSupportedInputFormat) override;
    HRESULT STDMETHODCALLTYPE IsOutputFormatSupported(IAudioMediaType* pOppositeFormat,
        IAudioMediaType* pRequestedOutputFormat, IAudioMediaType** ppSupportedOutputFormat) override;
    HRESULT STDMETHODCALLTYPE GetInputChannelCount(UINT32* pu32ChannelCount) override;

    // IAudioProcessingObjectRT (thread temps réel : ni allocation, ni verrou, ni appel système bloquant)
    void STDMETHODCALLTYPE APOProcess(UINT32 u32NumInputConnections, APO_CONNECTION_PROPERTY** ppInputConnections,
        UINT32 u32NumOutputConnections, APO_CONNECTION_PROPERTY** ppOutputConnections) override;
    UINT32 STDMETHODCALLTYPE CalcInputFrames(UINT32 u32OutputFrameCount) override { return u32OutputFrameCount; }
    UINT32 STDMETHODCALLTYPE CalcOutputFrames(UINT32 u32InputFrameCount) override { return u32InputFrameCount; }

    // IAudioProcessingObjectConfiguration
    HRESULT STDMETHODCALLTYPE LockForProcess(UINT32 u32NumInputConnections, APO_CONNECTION_DESCRIPTOR** ppInputConnections,
        UINT32 u32NumOutputConnections, APO_CONNECTION_DESCRIPTOR** ppOutputConnections) override;
    HRESULT STDMETHODCALLTYPE UnlockForProcess() override;

    // IAudioSystemEffects2
    HRESULT STDMETHODCALLTYPE GetEffectsList(LPGUID* ppEffectsIds, UINT* pcEffects, HANDLE Event) override;

private:
    HRESULT CheckFormat(IAudioMediaType* requested, IAudioMediaType** supported);
    void OpenSharedBuffer();
    void CloseSharedBuffer();
    void MixInjected(float* out, UINT32 frames);

    std::atomic<long> m_refCount{1};
    IUnknown* m_outer;  // objet extérieur, ou nous-même si non agrégé

    bool m_locked = false;
    UINT32 m_channels = 0;
    UINT32 m_rate = 0;

    HANDLE m_file = INVALID_HANDLE_VALUE;
    HANDLE m_mapping = nullptr;
    BYTE* m_view = nullptr;
    StreameeShmHeader* m_header = nullptr;
    const float* m_ring = nullptr;
    int64_t m_cursor = -1;  // prochaine trame de l'anneau à lire, -1 = pas encore synchronisé
};
