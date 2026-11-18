/* The configure-generated header for a Linux build, as FreeSWITCH's own sofia-sip build has it
   (sofia-sip-ua 1.12.0 binary compatibility, so isize_t is int). */
#ifndef SU_CONFIGURE_H
#define SU_CONFIGURE_H

#include <limits.h>

#define SU_HAVE_STDINT 1
#define SU_HAVE_INTTYPES 1
#define SU_HAVE_SYS_TYPES 1
#define SU_HAVE_INLINE 1
#define SU_INLINE inline
#define su_inline static inline

#define SOFIA_ISIZE_T int
#define ISIZE_MAX INT_MAX
#define SOFIA_ISSIZE_T int
#define ISSIZE_MAX INT_MAX
#define SOFIA_USIZE_T unsigned
#define USIZE_MAX UINT_MAX

#endif
