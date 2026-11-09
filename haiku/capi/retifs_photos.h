/*
 * retifs Photos' shared core, as C (haiku/capi): JSON in and out, images as
 * bytes. Strings are freed with rp_free(), images with rp_free_bytes().
 * rp_load() reads everything; the other calls name its photos and people by
 * ID, albums by their name in lower case. Answers that can fail are objects
 * with "error" when they did. Calls block.
 */
#ifndef RETIFS_PHOTOS_H
#define RETIFS_PHOTOS_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* {"libraries", "photos": [{"ns", "id", "title", "month", "favorite",
 *   "albums": [id], "people": [id]}], "albums": [{"id", "name", "cover"}],
 *  "people": [{"id", "name", "photo"}], "error"} */
char* rp_load(void);
/* {"title", "ns", "favorite", "details": [[label, value]],
 *  "people": [[id, name]], "albums": [id]} */
char* rp_details(const char* id);
/* The block for role ("thumb", "preview") of photo id, at most max bytes,
 * or NULL; its length in *len. */
uint8_t* rp_image(const char* id, const char* role, uint64_t max, size_t* len);
void rp_free_bytes(uint8_t* bytes, size_t len);

/* {} or {"error"} */
char* rp_set_favorite(const char* id, int on);
char* rp_tag(const char* id, const char* person, int on);
char* rp_file(const char* id, const char* album, int on);
char* rp_new_album(const char* name, const char* with_photo);
char* rp_new_person(const char* name, const char* tag_photo);

/* {ns: count}: reload when these differ */
char* rp_change_counts(void);

void rp_free(char* s);

#ifdef __cplusplus
}
#endif

#endif
