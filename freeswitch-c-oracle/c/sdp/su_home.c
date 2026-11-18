/* The three su_alloc calls sdp_parse.c makes, over calloc. A home owns every block allocated in it
   and frees them with itself; sdp_parse always allocates in the home su_home_clone returned. */
#include <stdlib.h>
#include <sofia-sip/su_alloc.h>

struct su_block_s {
	size_t count;
	size_t capacity;
	void **blocks;
};

void *su_home_clone(su_home_t *parent, isize_t size)
{
	su_home_t *home;

	(void) parent;
	if (size < (isize_t) sizeof(*home))
		size = sizeof(*home);
	home = calloc(1, size);
	if (!home)
		return NULL;
	home->suh_blocks = calloc(1, sizeof(*home->suh_blocks));
	if (!home->suh_blocks) {
		free(home);
		return NULL;
	}
	home->suh_size = size;
	return home;
}

void *su_salloc(su_home_t *home, isize_t size)
{
	struct { int size; } *retval;
	su_block_t *owned = home->suh_blocks;

	if (size < (isize_t) sizeof(*retval))
		size = sizeof(*retval);
	if (owned->count == owned->capacity) {
		size_t capacity = owned->capacity ? owned->capacity * 2 : 16;
		void **blocks = realloc(owned->blocks, capacity * sizeof(*blocks));
		if (!blocks)
			return NULL;
		owned->blocks = blocks;
		owned->capacity = capacity;
	}
	retval = calloc(1, size);
	if (!retval)
		return NULL;
	retval->size = size;
	owned->blocks[owned->count++] = retval;
	return retval;
}

int su_home_unref(su_home_t *home)
{
	su_block_t *owned = home->suh_blocks;

	for (size_t i = 0; i < owned->count; i++)
		free(owned->blocks[i]);
	free(owned->blocks);
	free(owned);
	free(home);
	return 1;
}
