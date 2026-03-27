import { useQuery } from '@tanstack/react-query';
import { fetchResources, type Resource } from '../api/client';

export function useResources(type?: string) {
  return useQuery<Resource[]>({
    queryKey: ['resources', type ?? 'all'],
    queryFn: () => fetchResources(type),
    staleTime: 30_000,
  });
}
