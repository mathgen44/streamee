#include "StreameeApo.h"

#include <string.h>

// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, recopié pour ne pas dépendre de ksmedia.h/ksguid.lib.
static const GUID kSubtypeFloat =
    { 0x00000003, 0x0000, 0x0010, { 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71 } };

std::atomic<long> g_objectCount{0};
std::atomic<long> g_lockCount{0};

StreameeApo::StreameeApo(IUnknown* outer)
    : m_outer(outer ? outer : reinterpret_cast<IUnknown*>(static_cast<INonDelegatingUnknown*>(this))) {
    ++g_objectCount;
}

StreameeApo::~StreameeApo() {
    CloseSharedBuffer();
    --g_objectCount;
}

// ---------------------------------------------------------------- COM

HRESULT STDMETHODCALLTYPE StreameeApo::NonDelegatingQueryInterface(REFIID riid, void** ppv) {
    if (!ppv) return E_POINTER;
    *ppv = nullptr;
    if (riid == __uuidof(IUnknown))
        *ppv = static_cast<INonDelegatingUnknown*>(this);
    else if (riid == __uuidof(IAudioProcessingObject))
        *ppv = static_cast<IAudioProcessingObject*>(this);
    else if (riid == __uuidof(IAudioProcessingObjectRT))
        *ppv = static_cast<IAudioProcessingObjectRT*>(this);
    else if (riid == __uuidof(IAudioProcessingObjectConfiguration))
        *ppv = static_cast<IAudioProcessingObjectConfiguration*>(this);
    else if (riid == __uuidof(IAudioSystemEffects))
        *ppv = static_cast<IAudioSystemEffects*>(this);
    else if (riid == __uuidof(IAudioSystemEffects2))
        *ppv = static_cast<IAudioSystemEffects2*>(this);
    else
        return E_NOINTERFACE;
    reinterpret_cast<IUnknown*>(*ppv)->AddRef();
    return S_OK;
}

ULONG STDMETHODCALLTYPE StreameeApo::NonDelegatingAddRef() { return ++m_refCount; }

ULONG STDMETHODCALLTYPE StreameeApo::NonDelegatingRelease() {
    long count = --m_refCount;
    if (count == 0) delete this;
    return count;
}

// ---------------------------------------------------------------- IAudioProcessingObject

HRESULT STDMETHODCALLTYPE StreameeApo::Reset() {
    m_cursor = -1;
    return S_OK;
}

HRESULT STDMETHODCALLTYPE StreameeApo::GetLatency(HNSTIME* pTime) {
    if (!pTime) return E_POINTER;
    *pTime = 0;
    return S_OK;
}

HRESULT STDMETHODCALLTYPE StreameeApo::GetRegistrationProperties(APO_REG_PROPERTIES** ppRegProps) {
    if (!ppRegProps) return E_POINTER;
    auto* props = static_cast<APO_REG_PROPERTIES*>(CoTaskMemAlloc(sizeof(APO_REG_PROPERTIES)));
    if (!props) return E_OUTOFMEMORY;
    ZeroMemory(props, sizeof(*props));
    props->clsid = CLSID_StreameeApo;
    props->Flags = static_cast<APO_FLAG>(APO_FLAG_DEFAULT);
    wcscpy_s(props->szFriendlyName, L"Streamee - injection micro");
    wcscpy_s(props->szCopyrightInfo, L"MIT - mathgen44");
    props->u32MajorVersion = 1;
    props->u32MinorVersion = 0;
    props->u32MinInputConnections = 1;
    props->u32MaxInputConnections = 1;
    props->u32MinOutputConnections = 1;
    props->u32MaxOutputConnections = 1;
    props->u32MaxInstances = 0xffffffff;
    props->u32NumAPOInterfaces = 1;
    props->iidAPOInterfaceList[0] = __uuidof(IAudioProcessingObject);
    *ppRegProps = props;
    return S_OK;
}

HRESULT STDMETHODCALLTYPE StreameeApo::Initialize(UINT32, BYTE*) {
    // Les données d'initialisation (APOInitSystemEffects/2) ne nous servent pas.
    return S_OK;
}

HRESULT StreameeApo::CheckFormat(IAudioMediaType* requested, IAudioMediaType** supported) {
    if (!requested || !supported) return E_POINTER;
    *supported = nullptr;
    UNCOMPRESSEDAUDIOFORMAT fmt;
    HRESULT hr = requested->GetUncompressedAudioFormat(&fmt);
    if (FAILED(hr)) return hr;
    // Le moteur audio de Windows travaille en float32 pour les effets système.
    if (fmt.guidFormatType != kSubtypeFloat || fmt.dwBytesPerSampleContainer != 4 || fmt.dwSamplesPerFrame == 0)
        return APOERR_FORMAT_NOT_SUPPORTED;
    requested->AddRef();
    *supported = requested;
    return S_OK;
}

HRESULT STDMETHODCALLTYPE StreameeApo::IsInputFormatSupported(IAudioMediaType*, IAudioMediaType* pRequestedInputFormat,
                                                              IAudioMediaType** ppSupportedInputFormat) {
    return CheckFormat(pRequestedInputFormat, ppSupportedInputFormat);
}

HRESULT STDMETHODCALLTYPE StreameeApo::IsOutputFormatSupported(IAudioMediaType*, IAudioMediaType* pRequestedOutputFormat,
                                                               IAudioMediaType** ppSupportedOutputFormat) {
    return CheckFormat(pRequestedOutputFormat, ppSupportedOutputFormat);
}

HRESULT STDMETHODCALLTYPE StreameeApo::GetInputChannelCount(UINT32* pu32ChannelCount) {
    if (!pu32ChannelCount) return E_POINTER;
    if (!m_locked) return APOERR_NOT_LOCKED;
    *pu32ChannelCount = m_channels;
    return S_OK;
}

// ---------------------------------------------------------------- IAudioProcessingObjectConfiguration

HRESULT STDMETHODCALLTYPE StreameeApo::LockForProcess(UINT32 u32NumInputConnections, APO_CONNECTION_DESCRIPTOR** ppInputConnections,
                                                      UINT32 u32NumOutputConnections, APO_CONNECTION_DESCRIPTOR** ppOutputConnections) {
    if (m_locked) return APOERR_ALREADY_LOCKED;
    if (u32NumInputConnections != 1 || u32NumOutputConnections != 1 || !ppInputConnections || !ppOutputConnections)
        return APOERR_NUM_CONNECTIONS_INVALID;

    UNCOMPRESSEDAUDIOFORMAT in, out;
    HRESULT hr = ppInputConnections[0]->pFormat->GetUncompressedAudioFormat(&in);
    if (FAILED(hr)) return hr;
    hr = ppOutputConnections[0]->pFormat->GetUncompressedAudioFormat(&out);
    if (FAILED(hr)) return hr;
    if (in.guidFormatType != kSubtypeFloat || out.guidFormatType != kSubtypeFloat ||
        in.dwSamplesPerFrame != out.dwSamplesPerFrame || in.fFramesPerSecond != out.fFramesPerSecond)
        return APOERR_FORMAT_NOT_SUPPORTED;

    m_channels = in.dwSamplesPerFrame;
    m_rate = static_cast<UINT32>(in.fFramesPerSecond + 0.5f);
    m_cursor = -1;
    OpenSharedBuffer();
    m_locked = true;
    return S_OK;
}

HRESULT STDMETHODCALLTYPE StreameeApo::UnlockForProcess() {
    if (!m_locked) return APOERR_NOT_LOCKED;
    m_locked = false;
    return S_OK;
}

// ---------------------------------------------------------------- IAudioSystemEffects2

HRESULT STDMETHODCALLTYPE StreameeApo::GetEffectsList(LPGUID* ppEffectsIds, UINT* pcEffects, HANDLE) {
    if (!ppEffectsIds || !pcEffects) return E_POINTER;
    *ppEffectsIds = nullptr;
    *pcEffects = 0;
    return S_OK;
}

// ---------------------------------------------------------------- Tampon partagé

void StreameeApo::OpenSharedBuffer() {
    if (m_view) return;
    // Absent (Streamee pas installé correctement) : on reste en simple recopie du micro.
    m_file = CreateFileW(STREAMEE_SHM_PATH, GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE,
                         nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (m_file == INVALID_HANDLE_VALUE) return;
    LARGE_INTEGER size;
    if (!GetFileSizeEx(m_file, &size) || size.QuadPart < (LONGLONG)STREAMEE_SHM_BYTES) { CloseSharedBuffer(); return; }
    m_mapping = CreateFileMappingW(m_file, nullptr, PAGE_READWRITE, 0, STREAMEE_SHM_BYTES, nullptr);
    if (!m_mapping) { CloseSharedBuffer(); return; }
    m_view = static_cast<BYTE*>(MapViewOfFile(m_mapping, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, STREAMEE_SHM_BYTES));
    if (!m_view) { CloseSharedBuffer(); return; }
    m_header = reinterpret_cast<StreameeShmHeader*>(m_view);
    if (m_header->magic != STREAMEE_MAGIC || m_header->version != STREAMEE_VERSION) { CloseSharedBuffer(); return; }
    m_ring = reinterpret_cast<const float*>(m_view + STREAMEE_HEADER_BYTES);
}

void StreameeApo::CloseSharedBuffer() {
    if (m_view) UnmapViewOfFile(m_view);
    if (m_mapping) CloseHandle(m_mapping);
    if (m_file != INVALID_HANDLE_VALUE) CloseHandle(m_file);
    m_view = nullptr;
    m_mapping = nullptr;
    m_file = INVALID_HANDLE_VALUE;
    m_header = nullptr;
    m_ring = nullptr;
}

static inline int64_t AtomicLoad64(volatile int64_t* p) {
    return InterlockedCompareExchange64(reinterpret_cast<volatile LONG64*>(p), 0, 0);
}

void StreameeApo::MixInjected(float* out, UINT32 frames) {
    StreameeShmHeader* h = m_header;
    volatile int64_t* readFrame = &h->read_frame;

    // Publie le format du micro et signale que l'APO tourne.
    InterlockedExchange(reinterpret_cast<volatile LONG*>(&h->apo_rate), (LONG)m_rate);
    InterlockedExchange(reinterpret_cast<volatile LONG*>(&h->apo_channels), (LONG)m_channels);
    InterlockedExchange64(reinterpret_cast<volatile LONG64*>(&h->apo_heartbeat), (LONG64)GetTickCount64());

    const int64_t written = AtomicLoad64(&h->write_frame);  // barrière complète : l'anneau est à jour jusqu'ici
    if (m_cursor < 0) {
        // Première lecture : on reprend là où en est l'instance la plus avancée,
        // sinon on se cale sur l'écriture (rien d'ancien n'est rejoué).
        int64_t shared = AtomicLoad64(readFrame);
        m_cursor = (shared > 0 && shared <= written && written - shared < (int64_t)STREAMEE_RING_FRAMES) ? shared : written;
    }

    const int64_t oldest = written - (int64_t)STREAMEE_RING_FRAMES;
    const UINT32 ch = m_channels;
    for (UINT32 i = 0; i < frames; ++i) {
        const int64_t f = m_cursor + i;
        if (f >= written || f < oldest) continue;  // pas encore écrit, ou déjà écrasé
        const float* src = m_ring + (size_t)(f & (STREAMEE_RING_FRAMES - 1)) * STREAMEE_RING_CHANNELS;
        float* dst = out + (size_t)i * ch;
        if (ch == 1) {
            dst[0] += 0.5f * (src[0] + src[1]);
        } else {
            dst[0] += src[0];
            dst[1] += src[1];
        }
    }
    m_cursor += frames;

    // read_frame = max(read_frame, curseur) — plusieurs instances peuvent lire en parallèle.
    int64_t seen = AtomicLoad64(readFrame);
    while (seen < m_cursor) {
        int64_t prev = InterlockedCompareExchange64(reinterpret_cast<volatile LONG64*>(readFrame), m_cursor, seen);
        if (prev == seen) break;
        seen = prev;
    }
}

// ---------------------------------------------------------------- IAudioProcessingObjectRT

void STDMETHODCALLTYPE StreameeApo::APOProcess(UINT32, APO_CONNECTION_PROPERTY** ppInputConnections,
                                               UINT32, APO_CONNECTION_PROPERTY** ppOutputConnections) {
    APO_CONNECTION_PROPERTY* in = ppInputConnections[0];
    APO_CONNECTION_PROPERTY* out = ppOutputConnections[0];
    const UINT32 frames = in->u32ValidFrameCount;
    float* dst = reinterpret_cast<float*>(out->pBuffer);
    const float* src = reinterpret_cast<const float*>(in->pBuffer);
    const size_t bytes = (size_t)frames * m_channels * sizeof(float);

    // 1) Le micro, tel quel (un tampon « silencieux » peut contenir n'importe quoi : on met des zéros).
    if (in->u32BufferFlags == BUFFER_VALID) {
        if (dst != src) memcpy(dst, src, bytes);
    } else {
        memset(dst, 0, bytes);
    }

    // 2) Les sons de Streamee par-dessus.
    if (m_ring && frames > 0) {
        MixInjected(dst, frames);
        out->u32BufferFlags = BUFFER_VALID;
    } else {
        out->u32BufferFlags = in->u32BufferFlags == BUFFER_VALID ? BUFFER_VALID : BUFFER_SILENT;
    }
    out->u32ValidFrameCount = frames;
}
