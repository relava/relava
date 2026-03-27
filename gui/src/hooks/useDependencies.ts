import { useQuery } from '@tanstack/react-query';
import { fetchDependencies, type ResolveResponse } from '../api/client';

export function useDependencies(
  type: string,
  name: string,
  version: string | undefined,
) {
  return useQuery<ResolveResponse>({
    queryKey: ['dependencies', type, name, version],
    queryFn: () => fetchDependencies(type, name, version),
    staleTime: 30_000,
    enabled: Boolean(type && name),
  });
}
