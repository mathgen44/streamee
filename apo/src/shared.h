// Streamee — disposition du tampon partagé entre l'application et l'APO.
// Toute modification doit être reportée dans src-tauri/src/inject.rs.
//
// Le tampon est un fichier projeté en mémoire (C:\ProgramData\Streamee\inject.shm),
// créé par le script d'installation avec les droits pour LOCAL SERVICE (audiodg.exe)
// et les utilisateurs authentifiés (l'application). Un fichier plutôt qu'un objet
// nommé « Global\ » : audiodg tourne en session 0 et l'application en session 1,
// et un utilisateur standard ne peut pas créer d'objet global.
//
// Synchronisation sans verrou :
//   - l'application écrit des trames stéréo float32 dans l'anneau, PUIS publie write_frame ;
//   - chaque instance de l'APO lit [curseur, curseur + n) en ignorant ce qui n'est pas
//     encore écrit ou déjà écrasé, et publie read_frame (maximum des instances) ;
//   - l'application garde write_frame environ 25 ms devant read_frame.
#pragma once
#include <stdint.h>

#define STREAMEE_SHM_PATH      L"C:\\ProgramData\\Streamee\\inject.shm"
#define STREAMEE_MAGIC         0x4D525453u  /* 'STRM' */
#define STREAMEE_VERSION       1u
#define STREAMEE_HEADER_BYTES  4096u
#define STREAMEE_RING_FRAMES   65536u       /* puissance de 2, ~1,36 s à 48 kHz */
#define STREAMEE_RING_CHANNELS 2u           /* l'anneau est toujours stéréo */
#define STREAMEE_SHM_BYTES     (STREAMEE_HEADER_BYTES + STREAMEE_RING_FRAMES * STREAMEE_RING_CHANNELS * 4u)

#pragma pack(push, 8)
typedef struct StreameeShmHeader {
    uint32_t magic;          /* 0  : STREAMEE_MAGIC (écrit par l'installeur) */
    uint32_t version;        /* 4  */
    uint32_t apo_rate;       /* 8  : fréquence du micro, publiée par l'APO */
    uint32_t apo_channels;   /* 12 : canaux du micro, publiés par l'APO */
    int64_t  apo_heartbeat;  /* 16 : GetTickCount64() du dernier APOProcess */
    int64_t  read_frame;     /* 24 : trame la plus avancée lue par l'APO */
    int64_t  write_frame;    /* 32 : première trame non encore écrite par l'application */
} StreameeShmHeader;
#pragma pack(pop)

/* L'anneau commence à STREAMEE_HEADER_BYTES : float ring[STREAMEE_RING_FRAMES * 2]. */
