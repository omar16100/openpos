/* openpos: the till behind a plain C ABI.
 *
 * Four functions. Every operation goes through openpos_till_run as a JSON
 * command, so adding one changes nothing here and nothing on the platform side
 * has to be regenerated.
 *
 * Rules the caller keeps:
 *   - a handle is used until openpos_till_close, and never after
 *   - a returned string is freed with openpos_string_free, never with free()
 *   - one handle is not used from two threads at once
 */
#ifndef OPENPOS_H
#define OPENPOS_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct OpenposTill OpenposTill;

/* Open a till whose data lives only in memory. Null if the ids are not ids. */
OpenposTill *openpos_till_open_memory(const char *tenant, const char *terminal);

/* Run one JSON command, get one JSON view. Never null. Free with
 * openpos_string_free. */
char *openpos_till_run(OpenposTill *till, const char *request);

/* Release a string this library returned. Null is harmless. */
void openpos_string_free(char *text);

/* Close a till. Null is harmless. Committed sales are already durable. */
void openpos_till_close(OpenposTill *till);

#ifdef __cplusplus
}
#endif
#endif /* OPENPOS_H */
