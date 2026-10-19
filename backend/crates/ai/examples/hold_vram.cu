// Reserva N MiB de VRAM y duerme (simula el uso de la GPU por el escritorio). Se para por PID.
#include <cstdio>
#include <cstdlib>
#include <unistd.h>
#include <cuda_runtime.h>
int main(int argc, char** argv) {
  size_t mib = argc > 1 ? atol(argv[1]) : 750;
  void* p = nullptr;
  if (cudaMalloc(&p, mib << 20) != cudaSuccess) { fprintf(stderr, "cudaMalloc falló\n"); return 1; }
  cudaMemset(p, 0, mib << 20);
  printf("reservados %zu MiB\n", mib); fflush(stdout);
  for (;;) sleep(3600);
}
