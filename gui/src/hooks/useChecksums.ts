import { useQuery } from '@tanstack/react-query';
import { fetchChecksums, type ChecksumsResponse } from '../api/client';

export function useChecksums(
  type: string,
  name: string,
  version: string | undefined,
) {
  return useQuery<ChecksumsResponse>({
    queryKey: ['checksums', type, name, version],
    queryFn: () => fetchChecksums(type, name, version!),
    staleTime: 30_000,
    enabled: Boolean(type && name && version),
  });
}
