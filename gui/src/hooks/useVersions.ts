import { useQuery } from '@tanstack/react-query';
import { fetchVersions, type Version } from '../api/client';

export function useVersions(type: string, name: string) {
  return useQuery<Version[]>({
    queryKey: ['versions', type, name],
    queryFn: () => fetchVersions(type, name),
    staleTime: 30_000,
    enabled: Boolean(type && name),
  });
}
