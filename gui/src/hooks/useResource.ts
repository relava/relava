import { useQuery } from '@tanstack/react-query';
import { fetchResource, type Resource } from '../api/client';

export function useResource(type: string, name: string) {
  return useQuery<Resource>({
    queryKey: ['resource', type, name],
    queryFn: () => fetchResource(type, name),
    staleTime: 30_000,
    enabled: Boolean(type && name),
  });
}
