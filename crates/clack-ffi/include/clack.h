#ifndef CLACK_H
#define CLACK_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ClackSession ClackSession;

/* Every `char*` this library returns must be released with clack_string_free.
   `error` may be NULL. When a call fails and `error` is set, it receives a
   newly allocated message. */

char *clack_normalize_model_ref(const char *input, char **error);

ClackSession *clack_create(const char *model, const char *language, const char *token, char **error);
void clack_free(ClackSession *session);

int clack_set_language(ClackSession *session, const char *language, char **error);
int clack_set_input_device(ClackSession *session, const char *device_id, char **error);

int clack_start_listening(ClackSession *session, char **error);
int clack_stop_listening(ClackSession *session, char **error);
int clack_cancel_listening(ClackSession *session, char **error);
int clack_push_audio(ClackSession *session, const float *samples, size_t count, uint32_t sample_rate, char **error);

int clack_is_listening(const ClackSession *session);
char *clack_transcript(const ClackSession *session);
char *clack_last_error(const ClackSession *session);
char *clack_repo_id(const ClackSession *session);

/* Returns 1 and fills the outputs when an update is waiting, otherwise 0.
   `text` is allocated. `is_final` and `cancelled` are 0 or 1. */
int clack_poll(ClackSession *session, int *is_final, int *cancelled, char **text);

typedef struct ClackActivity {
    int speaking;
    float levels[32];
} ClackActivity;

void clack_speech_activity(const ClackSession *session, ClackActivity *out);

int clack_input_device_count(char **error);
char *clack_input_device_id(int index, char **error);
char *clack_input_device_name(int index, char **error);
int clack_input_device_is_default(int index);

void clack_string_free(char *text);

#ifdef __cplusplus
}
#endif

#endif
