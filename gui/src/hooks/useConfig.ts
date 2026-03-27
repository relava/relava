import { useQuery } from '@tanstack/react-query';
import { fetchConfig, type ConfigResponse } from '../api/client';

export function useConfig() {
  return useQuery<ConfigResponse>({
    queryKey: ['config'],
    queryFn: fetchConfig,
    staleTime: 30_000,
  });
}
