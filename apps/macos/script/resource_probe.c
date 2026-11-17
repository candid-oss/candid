#include <libproc.h>
#include <sys/resource.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <mach/mach_time.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static uint64_t cpu_nanoseconds(uint64_t user, uint64_t system) {
    mach_timebase_info_data_t scale;
    mach_timebase_info(&scale);
    return (uint64_t)((long double)(user + system) * scale.numer / scale.denom);
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    if (strcmp(argv[1], "--self-test") == 0) {
        struct rusage_info_v2 before = {0}, after = {0};
        struct timespec start, end;
        mach_timebase_info_data_t scale;
        mach_timebase_info(&scale);
        proc_pid_rusage(getpid(), RUSAGE_INFO_V2, (rusage_info_t *)&before);
        clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &start);
        uint64_t until = mach_absolute_time() + (uint64_t)(250000000.0L * scale.denom / scale.numer);
        while (mach_absolute_time() < until) { }
        clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &end);
        proc_pid_rusage(getpid(), RUSAGE_INFO_V2, (rusage_info_t *)&after);
        double expected = (end.tv_sec-start.tv_sec)*1e9 + end.tv_nsec-start.tv_nsec;
        double measured = cpu_nanoseconds(after.ri_user_time, after.ri_system_time)-cpu_nanoseconds(before.ri_user_time, before.ri_system_time);
        double ratio = measured/expected;
        printf("{\"cpu_conversion_ratio\":%.6f,\"timebase_numer\":%u,\"timebase_denom\":%u,\"passed\":%s}\n", ratio, scale.numer, scale.denom, ratio>0.9 && ratio<1.1 ? "true" : "false");
        return ratio>0.9 && ratio<1.1 ? 0 : 1;
    }
    struct rusage_info_v6 info = {0};
    if (proc_pid_rusage(atoi(argv[1]), RUSAGE_INFO_V6, (rusage_info_t *)&info) != 0) return 1;
    printf("{\"cpu_ns\":%llu,\"footprint_bytes\":%llu,\"resident_bytes\":%llu,\"disk_written_bytes\":%llu,\"wakeups\":%llu,\"cpu_energy_nj\":%llu,\"performance_core_energy_nj\":%llu,\"disk_read_bytes\":%llu,\"interrupt_wakeups\":%llu,\"instructions\":%llu,\"cycles\":%llu,\"billed_energy_raw\":%llu,\"serviced_energy_raw\":%llu,\"billed_system_time_raw\":%llu,\"serviced_system_time_raw\":%llu}\n",
           (unsigned long long)cpu_nanoseconds(info.ri_user_time, info.ri_system_time),
           (unsigned long long)info.ri_phys_footprint,
           (unsigned long long)info.ri_resident_size,
           (unsigned long long)info.ri_diskio_byteswritten,
           (unsigned long long)info.ri_pkg_idle_wkups,
           (unsigned long long)info.ri_energy_nj,
           (unsigned long long)info.ri_penergy_nj,
           (unsigned long long)info.ri_diskio_bytesread,
           (unsigned long long)info.ri_interrupt_wkups,
           (unsigned long long)info.ri_instructions,
           (unsigned long long)info.ri_cycles,
           (unsigned long long)info.ri_billed_energy,
           (unsigned long long)info.ri_serviced_energy,
           (unsigned long long)info.ri_billed_system_time,
           (unsigned long long)info.ri_serviced_system_time);
    return 0;
}
